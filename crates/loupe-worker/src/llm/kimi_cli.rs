//! Backend that shells out to the `kimi` CLI (kimi-code).
//!
//! Runs `kimi -p "$prompt"` inside the bubblewrap sandbox, same shape
//! as the claude backend: the worktree is read-only at `/workdir`,
//! network is allowed so the CLI can reach its configured provider.
//!
//! Three kimi-specific differences from the claude backend:
//!
//! - kimi-code has no `--mcp-config` flag; it reads MCP servers from
//!   `~/.kimi-code/mcp.json` (same `mcpServers` JSON schema claude
//!   uses). The per-call scratch config is bind-mounted onto that
//!   fixed path under the sandbox HOME instead of passed as a flag.
//! - Model/provider configuration lives in kimi's own `config.toml`,
//!   bind-mounted read-only from the host dir [`kimi_home_dir`]
//!   resolves. `-m` selects a model *alias* defined there, not a raw
//!   provider slug. Auth arrives via the forwarded `OPENAI_API_KEY`
//!   (kimi's `openai`-type providers read it), so the config file can
//!   stay secret-free.
//! - Tool approval is not a flag. Print mode (`-p`) rejects `--yolo` /
//!   `--auto`, so autonomous tool execution (needed for the MCP
//!   `submit_finding` call) is driven by `default_permission_mode` in
//!   that same `config.toml` — set it to `"yolo"`, the sandbox being
//!   the security boundary rather than the CLI's permission system.

use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;

use super::mcp::{bind_mcp_into_sandbox, prepare_mcp_scratch, sandbox_workdir, McpContext};
use super::{kimi_home_dir, summarize_cli_stream_for_error, LlmBackend, LlmRequest, LlmResponse};
use crate::sandbox::SandboxBuilder;

const BACKEND_ID: &str = "kimi-cli";
const KIMI_BIN: &str = "kimi";
pub const DEFAULT_KIMI_MODEL: &str = "kimi-k3";
const MAX_CLI_DIAGNOSTIC_CHARS: usize = 2_000;

/// Fixed sandbox paths under the sandbox HOME (`/home/scanner`).
/// kimi-code discovers both by convention; there are no CLI flags.
/// The parent directory stays writable tmpfs so kimi can persist its
/// per-session state next to the two read-only mounts.
const SANDBOX_KIMI_CONFIG: &str = "/home/scanner/.kimi-code/config.toml";
const SANDBOX_KIMI_MCP_CONFIG: &str = "/home/scanner/.kimi-code/mcp.json";

pub struct KimiCliBackend {
	bin: String,
	model: String,
	mcp: Option<McpContext>,
	log_agent_output: bool,
	#[cfg(test)]
	disable_sandbox: bool,
}

impl KimiCliBackend {
	pub fn new() -> Self {
		Self {
			bin: KIMI_BIN.to_owned(),
			model: DEFAULT_KIMI_MODEL.to_owned(),
			mcp: None,
			log_agent_output: false,
			#[cfg(test)]
			disable_sandbox: false,
		}
	}

	#[cfg(test)]
	pub fn with_bin(bin: impl Into<String>) -> Self {
		Self { bin: bin.into(), ..Self::new() }
	}

	/// Model *alias* passed as `-m`. Must match a `[models.<alias>]`
	/// entry in the kimi `config.toml` this backend bind-mounts.
	pub fn with_model(mut self, model: impl Into<String>) -> Self {
		self.model = model.into();
		self
	}

	pub fn with_log_agent_output(mut self, enabled: bool) -> Self {
		self.log_agent_output = enabled;
		self
	}

	#[cfg(test)]
	fn with_sandbox_disabled_for_tests(mut self) -> Self {
		self.disable_sandbox = true;
		self
	}

	/// Attach an MCP server to every invocation. When set, each call
	/// writes the scratch `mcpServers` config and bind-mounts it at
	/// the fixed `~/.kimi-code/mcp.json` path kimi reads on startup.
	pub fn with_mcp_context(mut self, mcp: McpContext) -> Self {
		self.mcp = Some(mcp);
		self
	}
}

impl Default for KimiCliBackend {
	fn default() -> Self {
		Self::new()
	}
}

#[async_trait]
impl LlmBackend for KimiCliBackend {
	fn id(&self) -> &'static str {
		BACKEND_ID
	}

	async fn run(&self, req: LlmRequest) -> Result<LlmResponse> {
		tracing::debug!(
			backend = BACKEND_ID,
			workdir = %req.workdir.display(),
			model = %self.model,
			prompt_chars = req.prompt.chars().count(),
			timeout_ms = req.timeout.as_millis() as u64,
			"kimi-cli: invoking",
		);
		let started = std::time::Instant::now();

		#[cfg(test)]
		let sandbox_builder = if self.disable_sandbox {
			SandboxBuilder::disabled_for_tests(&req.workdir)
		} else {
			SandboxBuilder::new(&req.workdir)
		};
		#[cfg(not(test))]
		let sandbox_builder = SandboxBuilder::new(&req.workdir);

		let mut sandbox = sandbox_builder
			.allow_network()
			.allow_binary(&self.bin)
			.with_context(|| format!("preparing sandbox for `{}`", self.bin))?
			// Providers of type `openai` read this for auth; the
			// bound config.toml stays secret-free.
			.forward_env("OPENAI_API_KEY");
		if let Some(kimi_dir) = kimi_home_dir() {
			// Bind only config.toml, not the whole dir: kimi writes
			// session state under its home on every invocation, and a
			// read-only parent would fail those writes with EROFS.
			// `--ro-bind-try` semantics make a missing source a no-op.
			sandbox = sandbox.bind_ro(kimi_dir.join("config.toml"), SANDBOX_KIMI_CONFIG);
		}

		// Optional MCP attachment. Held in a local so its `TempDir`
		// lives until after the subprocess returns — dropping it
		// early would unlink the config file out from under kimi.
		let _mcp_scratch = match (&self.mcp, req.repo_id) {
			(Some(ctx), Some(repo_id)) => {
				let workdir = sandbox_workdir(&req.workdir);
				let scratch =
					prepare_mcp_scratch(ctx, repo_id, req.job_id, req.finding_id, &workdir)
						.context("preparing MCP scratch directory")?;
				sandbox = bind_mcp_into_sandbox(sandbox, ctx)
					.bind_ro(scratch.config_path.clone(), SANDBOX_KIMI_MCP_CONFIG);
				Some(scratch)
			},
			(Some(_), None) => {
				tracing::debug!(
					backend = BACKEND_ID,
					"MCP context configured but request has no repo_id; skipping MCP config",
				);
				None
			},
			_ => None,
		};

		let mut cmd = sandbox.build(&self.bin);
		for arg in kimi_invocation_args(&self.model, &req.prompt) {
			cmd.arg(arg);
		}
		cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
		cmd.kill_on_drop(true);

		let mut child = cmd
			.spawn()
			.with_context(|| format!("spawning `{}` (is the kimi CLI installed?)", self.bin))?;

		let stdout_handle = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
		let stderr_handle = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

		let cancel = req.cancel.clone();
		let wait_fut = async move {
			tokio::select! {
				biased;
				_ = cancel.cancelled() => {
					let _ = child.kill().await;
					Err(anyhow!("cancelled"))
				}
				res = child.wait() => res.map_err(Into::into),
			}
		};

		let (status, stdout, stderr) = match timeout(req.timeout, async {
			let mut stdout_buf = Vec::new();
			let mut stderr_buf = Vec::new();
			let mut so = stdout_handle;
			let mut se = stderr_handle;
			let (status, _, _) = tokio::join!(
				wait_fut,
				so.read_to_end(&mut stdout_buf),
				se.read_to_end(&mut stderr_buf),
			);
			Result::<_>::Ok((status?, stdout_buf, stderr_buf))
		})
		.await
		{
			Ok(inner) => inner?,
			Err(_) => return Err(anyhow!("kimi CLI timed out after {:?}", req.timeout)),
		};

		if !status.success() {
			let stderr_text = String::from_utf8_lossy(&stderr);
			let stdout_text = String::from_utf8_lossy(&stdout);
			tracing::debug!(
				backend = BACKEND_ID,
				exit = ?status.code(),
				stdout_chars = stdout.len(),
				stderr_chars = stderr.len(),
				elapsed_ms = started.elapsed().as_millis() as u64,
				"kimi-cli: subprocess failed",
			);
			// Surface both streams: CLIs routinely print auth errors
			// to stdout. Trim and truncate so a multi-MB diagnostic
			// dump doesn't drown the log line.
			let combined = format!(
				"stderr(chars={})=`{}` stdout(chars={})=`{}`",
				stderr_text.chars().count(),
				summarize_cli_stream_for_error(&stderr_text, MAX_CLI_DIAGNOSTIC_CHARS),
				stdout_text.chars().count(),
				summarize_cli_stream_for_error(&stdout_text, MAX_CLI_DIAGNOSTIC_CHARS),
			);
			return Err(anyhow!("kimi CLI exited with {}: {}", status, combined));
		}

		let text =
			String::from_utf8(stdout).map_err(|e| anyhow!("kimi CLI stdout was not UTF-8: {e}"))?;
		if self.log_agent_output {
			tracing::info!(
				backend = BACKEND_ID,
				agent_stdout = %text,
				"kimi-cli: agent stdout (full)"
			);
			if !stderr.is_empty() {
				let stderr_text = String::from_utf8_lossy(&stderr);
				tracing::info!(
					backend = BACKEND_ID,
					agent_stderr = %stderr_text,
					"kimi-cli: agent stderr (full)"
				);
			}
		}
		tracing::debug!(
			backend = BACKEND_ID,
			elapsed_ms = started.elapsed().as_millis() as u64,
			stdout_chars = text.chars().count(),
			stderr_chars = stderr.len(),
			"kimi-cli: subprocess succeeded",
		);
		Ok(LlmResponse { text, backend_id: BACKEND_ID })
	}
}

fn kimi_invocation_args(model: &str, prompt: &str) -> Vec<String> {
	// No `-y`/`--auto`: print mode rejects both. Tool approval comes from
	// `default_permission_mode` in the bound config.toml. No effort flag:
	// kimi-code exposes no effort control.
	vec![
		"--output-format".to_owned(),
		"text".to_owned(),
		"-m".to_owned(),
		model.to_owned(),
		"-p".to_owned(),
		prompt.to_owned(),
	]
}

#[cfg(test)]
mod tests {
	use std::time::Duration;

	use tokio_util::sync::CancellationToken;

	use super::*;

	#[test]
	fn invocation_args_shape() {
		let args = kimi_invocation_args("kimi-k3", "find bugs");
		assert_eq!(args, vec!["--output-format", "text", "-m", "kimi-k3", "-p", "find bugs"]);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn fake_cli_round_trip_captures_stdout() {
		use std::os::unix::fs::PermissionsExt;

		let scratch = tempfile::tempdir().unwrap();
		let bin = scratch.path().join("fake-kimi");
		std::fs::write(&bin, "#!/bin/sh\necho kimi-stdout-marker\n").unwrap();
		std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

		let workdir = tempfile::tempdir().unwrap();
		let backend =
			KimiCliBackend::with_bin(bin.to_string_lossy()).with_sandbox_disabled_for_tests();
		let req = LlmRequest {
			prompt: "irrelevant".into(),
			workdir: workdir.path().to_path_buf(),
			timeout: Duration::from_secs(5),
			cancel: CancellationToken::new(),
			repo_id: None,
			job_id: None,
			finding_id: None,
		};
		let resp = backend.run(req).await.expect("fake CLI must succeed");
		assert!(resp.text.contains("kimi-stdout-marker"), "got: {}", resp.text);
		assert_eq!(resp.backend_id, "kimi-cli");
	}

	#[tokio::test]
	async fn missing_binary_errors_clearly() {
		let workdir = tempfile::tempdir().unwrap();
		let backend = KimiCliBackend::with_bin("loupe-test-definitely-not-a-real-kimi");
		let req = LlmRequest {
			prompt: "irrelevant".into(),
			workdir: workdir.path().to_path_buf(),
			timeout: Duration::from_secs(5),
			cancel: CancellationToken::new(),
			repo_id: None,
			job_id: None,
			finding_id: None,
		};
		let err = backend.run(req).await.expect_err("must error");
		// `allow_binary` fails on the PATH lookup before spawn is ever
		// reached; either layer's message names the missing binary.
		// Don't be picky about which one fired.
		assert!(
			err.to_string().contains("loupe-test-definitely-not-a-real-kimi"),
			"unexpected error: {err}"
		);
	}
}
