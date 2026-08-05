//! Shared MCP-attachment plumbing for both LLM backends.
//!
//! Both `claude` and `codex` need to advertise the same loupe MCP
//! server (and optionally bkb-mcp) to the agent at invocation time.
//! The sandbox-side paths are identical across backends — only the
//! mechanism for *telling the CLI to load the config* differs:
//!
//! - claude takes `--mcp-config <file>` pointing at a JSON file the
//!   per-call scratch dir holds.
//! - codex takes `-c mcp_servers.<name>.command="..."` repeated for
//!   each TOML key under `mcp_servers.<name>`; no scratch file.
//!
//! Each backend wraps the same args list (this module's
//! [`mcp_serve_args`]) into its CLI's preferred shape. The sandbox
//! bind-mounts and cert paths are identical, so [`bind_mcp_into_sandbox`]
//! does that work in one place.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::sandbox::SandboxBuilder;

/// Fixed sandbox paths the MCP child resolves at runtime. Inside the
/// bwrap sandbox the agent only ever sees these, regardless of
/// where the host install actually lives — the bind mounts in
/// [`bind_mcp_into_sandbox`] keep the abstraction watertight.
pub const SANDBOX_LOUPE_BIN: &str = "/loupe/loupe-worker";
pub const SANDBOX_CA_CERT: &str = "/loupe/ca.pem";
pub const SANDBOX_CLIENT_CERT: &str = "/loupe/worker.pem";
pub const SANDBOX_CLIENT_KEY: &str = "/loupe/worker.key";
pub const SANDBOX_BKB_MCP_BIN: &str = "/loupe/bkb-mcp";

/// Default BKB HTTP API endpoint for the bkb-mcp child.
///
/// bkb-mcp's own compiled-in default (`http://127.0.0.1:3000`) is
/// handy for a developer running the BKB stack locally but useless
/// on a fresh worker host that has only `cargo install`'d the
/// client. Loupe overrides unconditionally so the bkb tools work
/// out of the box pointing at the public hosted instance, with
/// uniform behaviour across the worker fleet.
///
/// Operators with a self-hosted BKB instance can override this through
/// the worker config.
pub const DEFAULT_BKB_API_URL: &str = "https://bitcoinknowledge.dev";

/// Everything the MCP child needs to talk back to loupe-server.
/// Built once at worker startup from the `loupe-worker run` CLI
/// flags and stashed on the backend; per-call data (the repo id /
/// job id) arrives through [`super::LlmRequest`].
#[derive(Debug, Clone)]
pub struct McpContext {
	/// Path to the loupe-worker binary on the host. Usually
	/// `std::env::current_exe()` for the worker itself, so the same
	/// binary serves both `run` and `mcp-serve` modes.
	pub worker_binary: PathBuf,
	/// loupe-server URL the MCP child will call back to.
	pub server_url: String,
	pub tls: McpTlsSource,
	/// Optional `bkb-mcp` binary path. When `Some`, the per-call MCP
	/// config gets a second server entry exposing bkb's spec /
	/// historical-context tools (`bkb_search`, `bkb_lookup_bip`, …)
	/// alongside loupe's `submit_finding`. None means "host doesn't
	/// have bkb-mcp installed; advertise loupe only."
	pub bkb_mcp_path: Option<PathBuf>,
	/// HTTP API endpoint for the optional bkb-mcp child.
	pub bkb_api_url: String,
}

#[derive(Debug, Clone)]
pub enum McpTlsSource {
	Paths { ca_cert_path: PathBuf, client_cert_path: PathBuf, client_key_path: PathBuf },
	Env,
}

/// Build the args list that gets appended to `loupe-worker
/// mcp-serve` for one MCP-attached agent invocation. Cert + binary
/// paths come from the sandbox fixed paths above; per-call data
/// (`repo_id`, `job_id`, `finding_id`, `sandbox_workdir`) is wired
/// by the caller.
///
/// `job_id` is optional — the MCP server hides `submit_finding` when
/// it isn't supplied (e.g. a future read-only diagnostic flow).
/// `finding_id`, when present, flips the MCP server into verify
/// mode: it advertises `submit_verdict` / `submit_patch` /
/// `validate_patch` instead of `submit_finding` / `validate_poc`.
/// Both backends emit the same args list; only the wrapper around
/// it (a JSON file vs. `-c` overrides) differs.
pub fn mcp_serve_args(
	ctx: &McpContext, repo_id: i64, job_id: Option<i64>, finding_id: Option<i64>,
	sandbox_workdir: &str,
) -> Vec<String> {
	let mut args: Vec<String> = vec![
		"mcp-serve".into(),
		"--server-url".into(),
		ctx.server_url.clone(),
		"--repo-id".into(),
		repo_id.to_string(),
		"--workdir".into(),
		sandbox_workdir.to_owned(),
	];
	if matches!(ctx.tls, McpTlsSource::Paths { .. }) {
		args.splice(
			3..3,
			[
				"--ca-cert".into(),
				SANDBOX_CA_CERT.into(),
				"--cert".into(),
				SANDBOX_CLIENT_CERT.into(),
				"--key".into(),
				SANDBOX_CLIENT_KEY.into(),
			],
		);
	}
	if let Some(j) = job_id {
		args.push("--job-id".into());
		args.push(j.to_string());
	}
	if let Some(f) = finding_id {
		args.push("--finding-id".into());
		args.push(f.to_string());
	}
	args
}

/// The worktree path as the MCP child sees it: `/workdir` inside
/// bwrap; the host path in dev-only `LOUPE_DISABLE_SANDBOX` mode,
/// where the child shares the worker's filesystem view.
pub fn sandbox_workdir(host_workdir: &Path) -> String {
	if std::env::var_os(crate::sandbox::DISABLE_SANDBOX_ENV).is_some_and(|v| !v.is_empty()) {
		host_workdir.to_string_lossy().into_owned()
	} else {
		"/workdir".to_owned()
	}
}

/// Per-call MCP scratch: a host-side tempdir holding the JSON config
/// (Claude Code's `mcpServers` schema, which kimi-code also reads).
/// The `TempDir` is returned so the caller keeps it alive until after
/// the agent exits; dropping it unlinks the config file.
pub struct McpScratch {
	#[allow(dead_code)] // RAII — drop at end of caller's scope cleans up.
	dir: tempfile::TempDir,
	pub config_path: PathBuf,
}

pub fn prepare_mcp_scratch(
	ctx: &McpContext, repo_id: i64, job_id: Option<i64>, finding_id: Option<i64>,
	sandbox_workdir: &str,
) -> Result<McpScratch> {
	let dir = tempfile::Builder::new()
		.prefix("loupe-mcp-")
		.tempdir()
		.context("creating MCP scratch tempdir")?;
	let config_path = dir.path().join("mcp-config.json");
	let args = mcp_serve_args(ctx, repo_id, job_id, finding_id, sandbox_workdir);
	let mut servers = serde_json::Map::new();
	servers.insert(
		"loupe".to_string(),
		serde_json::json!({
			"type": "stdio",
			// Inside the sandbox the worker binary is mounted at
			// SANDBOX_LOUPE_BIN, the cert files under /loupe/...
			// — see [`bind_mcp_into_sandbox`].
			"command": SANDBOX_LOUPE_BIN,
			"args": args,
			// The MCP child inherits the bwrap'd env (sandbox HOME
			// plus the backend's forwarded auth vars — irrelevant
			// for the MCP child but harmless). No extra env needed
			// at this layer.
			"env": {}
		}),
	);
	// Conditionally attach bkb-mcp. The binary is bind-mounted under
	// /loupe/bkb-mcp by the caller. bkb-mcp itself is a thin client
	// to the BKB HTTP API: we always override its compiled-in
	// localhost default to the worker-configured API URL by setting
	// `BKB_API_URL` in the per-MCP `env` block — that's
	// MCP-server-scoped, doesn't leak into the agent or other
	// potential sibling MCP children.
	if ctx.bkb_mcp_path.is_some() {
		servers.insert(
			"bkb".to_string(),
			serde_json::json!({
				"type": "stdio",
				"command": SANDBOX_BKB_MCP_BIN,
				"args": [],
				"env": { "BKB_API_URL": ctx.bkb_api_url.as_str() }
			}),
		);
	}
	let config = serde_json::json!({ "mcpServers": servers });
	std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)
		.with_context(|| format!("writing MCP config at {}", config_path.display()))?;
	tracing::debug!(
		config_path = %config_path.display(),
		repo_id,
		job_id = ?job_id,
		"loupe-mcp: prepared per-call scratch config",
	);
	Ok(McpScratch { dir, config_path })
}

/// Bind the worker binary, mTLS cert/key/CA, and (optionally) the
/// bkb-mcp binary into the sandbox at the fixed paths above. Idempotent
/// across both backends — same mounts, same paths.
pub fn bind_mcp_into_sandbox(sandbox: SandboxBuilder, ctx: &McpContext) -> SandboxBuilder {
	let mut sb = sandbox.bind_ro(ctx.worker_binary.clone(), SANDBOX_LOUPE_BIN);
	match &ctx.tls {
		McpTlsSource::Paths { ca_cert_path, client_cert_path, client_key_path } => {
			sb = sb
				.bind_ro(ca_cert_path.clone(), SANDBOX_CA_CERT)
				.bind_ro(client_cert_path.clone(), SANDBOX_CLIENT_CERT)
				.bind_ro(client_key_path.clone(), SANDBOX_CLIENT_KEY);
		},
		McpTlsSource::Env => {
			sb = sb
				.forward_env("LOUPE_WORKER_CA_CERT_PEM")
				.forward_env("LOUPE_WORKER_CA_CERT_PEM_B64")
				.forward_env("LOUPE_WORKER_CERT_PEM")
				.forward_env("LOUPE_WORKER_CERT_PEM_B64")
				.forward_env("LOUPE_WORKER_KEY_PEM");
			sb = sb.forward_env("LOUPE_WORKER_KEY_PEM_B64");
		},
	}
	if let Some(bkb_path) = &ctx.bkb_mcp_path {
		sb = sb.bind_ro(bkb_path.clone(), SANDBOX_BKB_MCP_BIN);
	}
	sb
}
