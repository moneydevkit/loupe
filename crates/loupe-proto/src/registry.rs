use loupe_core::ReportingDestination;
use serde::{Deserialize, Serialize};

use crate::version::PROTOCOL_VERSION;

/// Wire-only reporting setup. Carries the GitHub PAT inline so the
/// admin can register a repo in a single round-trip; the server moves
/// the PAT into the `secrets` table and persists a
/// `loupe_core::ReportingDestination` referencing the resulting
/// `pat_secret_id`. PAT material never travels back out of the server
/// in any response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportingSetup {
	GithubIssue {
		target_owner: String,
		target_repo: String,
		github_pat: String,
	},
	/// Send findings as email via the server's `sendmail` binary. No
	/// secret material is required — the binary handles transport.
	Email {
		to: Vec<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		from: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		subject_prefix: Option<String>,
	},
	/// No automatic reporter. The server runs the scan, verification,
	/// and approval pipeline as usual, but confirmed findings remain
	/// `confirmed` until an operator configures reporting and retries
	/// delivery or handles them out-of-band.
	Manual,
}

/// Body of `POST /v1/repos`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRepoRequest {
	pub protocol_version: u16,
	pub clone_url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub branch: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scan_interval_seconds: Option<u64>,
	pub reporting: ReportingSetup,
	#[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
	pub scanner_config: serde_json::Value,
	/// Per-repo override of the verify flow. `None` (the default on
	/// the wire) means "inherit the server's
	/// `verification_default`". `Some(true)` / `Some(false)`
	/// pin the value for this repo regardless of the server default.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub verification_enabled: Option<bool>,
	/// Per-repo override of the human-in-the-loop approval gate. `None`
	/// (the default on the wire) means "inherit the server's
	/// `require_approval_default`". `Some(true)` / `Some(false)` pin
	/// the value for this repo regardless of the server default.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub require_approval: Option<bool>,
}

impl RegisterRepoRequest {
	pub fn new(clone_url: impl Into<String>, reporting: ReportingSetup) -> Self {
		Self {
			protocol_version: PROTOCOL_VERSION,
			clone_url: clone_url.into(),
			branch: None,
			scan_interval_seconds: None,
			reporting,
			scanner_config: serde_json::Value::Null,
			verification_enabled: None,
			require_approval: None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRepoResponse {
	pub protocol_version: u16,
	pub repo_id: i64,
}

/// Body of `POST /v1/repos/:id/reporting/github-pat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RotateRepoPatRequest {
	pub protocol_version: u16,
	pub github_pat: String,
}

/// Body of `PUT /v1/repos/:id/reporting/github`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRepoGithubReportingRequest {
	pub protocol_version: u16,
	pub target_owner: String,
	pub target_repo: String,
	pub github_pat: String,
}

/// Body of `PATCH /v1/repos/:id`. All fields are optional — only the
/// ones present in the JSON are applied. `disabled = Some(true)` stamps
/// `disabled_at = now`; `disabled = Some(false)` clears it. The repo's
/// reporting destination, clone URL, and PAT cannot be patched: those
/// are register-time inputs, and changing them would silently affect
/// where new findings get filed. Re-register the repo for that.
///
/// `require_approval` is tri-state on the wire: omitted = leave the
/// existing per-repo override alone; `Some(true)` / `Some(false)` =
/// pin per-repo. To clear the per-repo override back to "inherit
/// server default", set `inherit_require_approval = true` instead.
/// The server rejects requests that set both at once.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateRepoRequest {
	pub protocol_version: u16,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub disabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scan_interval_seconds: Option<u64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub verification_enabled: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub require_approval: Option<bool>,
	#[serde(default, skip_serializing_if = "std::ops::Not::not")]
	pub inherit_require_approval: bool,
}

/// Response body of `GET /v1/repos`. `RepoSummary` carries a sanitized
/// [`ReportingSummary`] rather than the storage-only
/// `loupe_core::ReportingDestination`, so clients learn *which* reporter
/// a repo uses without ever seeing `pat_secret_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListReposResponse {
	pub protocol_version: u16,
	pub repos: Vec<RepoSummary>,
}

/// Non-secret view of a repo's reporting destination.
///
/// Mirrors `loupe_core::ReportingDestination` minus `pat_secret_id`.
/// Clients need the reporter *kind* to decide which actions apply — PAT
/// rotation is only meaningful for `GithubIssue`, and the server 400s a
/// rotation attempt against any other destination — but the storage-side
/// secret id has no meaning off the server and must never leave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportingSummary {
	GithubIssue {
		target_owner: String,
		target_repo: String,
	},
	Email {
		to: Vec<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		from: Option<String>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		subject_prefix: Option<String>,
	},
	Manual,
}

impl From<&ReportingDestination> for ReportingSummary {
	fn from(dest: &ReportingDestination) -> Self {
		// Destructured field-by-field on purpose: no `..` rest pattern, so
		// adding a field to `ReportingDestination` fails to compile here
		// and forces a deliberate "does this belong on the wire?" call
		// rather than silently leaking or silently dropping it.
		match dest {
			ReportingDestination::GithubIssue { target_owner, target_repo, pat_secret_id: _ } => {
				Self::GithubIssue {
					target_owner: target_owner.clone(),
					target_repo: target_repo.clone(),
				}
			},
			ReportingDestination::Email { to, from, subject_prefix } => Self::Email {
				to: to.clone(),
				from: from.clone(),
				subject_prefix: subject_prefix.clone(),
			},
			ReportingDestination::Manual => Self::Manual,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSummary {
	pub id: i64,
	pub clone_url: String,
	pub host: String,
	pub owner: String,
	pub repo: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub default_branch: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub scan_interval_seconds: Option<i64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub disabled_at: Option<i64>,
	pub verification_enabled: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub require_approval: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_scanned_sha: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_scanned_at: Option<i64>,
	pub created_at: i64,
	/// Which reporter this repo is configured for, without secret
	/// material. `Option` because the field is additive: a client built
	/// against a newer protocol must still deserialize responses from a
	/// server that predates it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reporting: Option<ReportingSummary>,
}

/// Body of `POST /v1/workers` (admin-only). Returns the freshly-minted
/// client cert + key + the CA cert; this is the **only** time the client
/// key leaves the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterWorkerRequest {
	pub protocol_version: u16,
	pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterWorkerResponse {
	pub protocol_version: u16,
	pub worker_id: i64,
	pub client_cert_pem: String,
	pub client_key_pem: String,
	pub ca_cert_pem: String,
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn register_repo_request_round_trips() {
		let req = RegisterRepoRequest {
			protocol_version: PROTOCOL_VERSION,
			clone_url: "https://github.com/acme/widget.git".into(),
			branch: Some("main".into()),
			scan_interval_seconds: Some(3600),
			reporting: ReportingSetup::GithubIssue {
				target_owner: "acme".into(),
				target_repo: "security".into(),
				github_pat: "ghp_xxx".into(),
			},
			scanner_config: json!({"regex": {"enabled": true}}),
			verification_enabled: Some(true),
			require_approval: Some(false),
		};
		let s = serde_json::to_string(&req).unwrap();
		let back: RegisterRepoRequest = serde_json::from_str(&s).unwrap();
		assert_eq!(req, back);
		// Sanity check: the wire form does not leak `pat_secret_id`.
		assert!(!s.contains("pat_secret_id"));
	}

	#[test]
	fn register_repo_request_omits_inherited_verification_default() {
		let req =
			RegisterRepoRequest::new("https://github.com/acme/widget.git", ReportingSetup::Manual);
		let s = serde_json::to_string(&req).unwrap();
		assert!(
			!s.contains("verification_enabled"),
			"verification default should be inherited unless pinned: {s}"
		);
		let back: RegisterRepoRequest = serde_json::from_str(&s).unwrap();
		assert_eq!(back.verification_enabled, None);
	}

	fn summary_with_reporting(reporting: Option<ReportingSummary>) -> RepoSummary {
		RepoSummary {
			id: 1,
			clone_url: "https://github.com/acme/widget.git".into(),
			host: "github.com".into(),
			owner: "acme".into(),
			repo: "widget".into(),
			default_branch: Some("main".into()),
			scan_interval_seconds: Some(3600),
			disabled_at: None,
			verification_enabled: true,
			require_approval: None,
			last_scanned_sha: None,
			last_scanned_at: None,
			created_at: 42,
			reporting,
		}
	}

	#[test]
	fn reporting_summary_drops_the_pat_secret_id() {
		let dest = ReportingDestination::GithubIssue {
			target_owner: "acme".into(),
			target_repo: "tracker".into(),
			pat_secret_id: 7,
		};
		let summary = ReportingSummary::from(&dest);
		assert_eq!(
			summary,
			ReportingSummary::GithubIssue {
				target_owner: "acme".into(),
				target_repo: "tracker".into(),
			}
		);

		// The whole point of the type: the storage-side secret id must not
		// survive the conversion onto the wire.
		let s = serde_json::to_string(&summary).unwrap();
		assert!(s.contains(r#""kind":"github_issue""#), "kind must be tagged: {s}");
		assert!(s.contains("acme"), "non-secret target must survive: {s}");
		assert!(!s.contains("pat_secret_id"), "secret id leaked onto the wire: {s}");
		assert!(!s.contains('7'), "secret id value leaked onto the wire: {s}");
	}

	#[test]
	fn reporting_summary_preserves_email_and_manual() {
		let email = ReportingDestination::Email {
			to: vec!["sec@acme.test".into()],
			from: Some("loupe@acme.test".into()),
			subject_prefix: Some("[loupe]".into()),
		};
		assert_eq!(
			ReportingSummary::from(&email),
			ReportingSummary::Email {
				to: vec!["sec@acme.test".into()],
				from: Some("loupe@acme.test".into()),
				subject_prefix: Some("[loupe]".into()),
			}
		);
		assert_eq!(ReportingSummary::from(&ReportingDestination::Manual), ReportingSummary::Manual);
	}

	#[test]
	fn repo_summary_round_trips_with_reporting() {
		let summary = summary_with_reporting(Some(ReportingSummary::GithubIssue {
			target_owner: "acme".into(),
			target_repo: "tracker".into(),
		}));
		let s = serde_json::to_string(&summary).unwrap();
		let back: RepoSummary = serde_json::from_str(&s).unwrap();
		assert_eq!(summary, back);
		assert!(!s.contains("pat_secret_id"));
	}

	#[test]
	fn repo_summary_tolerates_a_response_without_reporting() {
		// `reporting` is additive, so a client built against this protocol
		// must still read a response from a server that never sends it.
		let summary = summary_with_reporting(None);
		let s = serde_json::to_string(&summary).unwrap();
		assert!(!s.contains("reporting"), "absent reporting should not be serialized: {s}");
		let back: RepoSummary = serde_json::from_str(&s).unwrap();
		assert_eq!(back.reporting, None);
	}

	#[test]
	fn register_worker_response_carries_pem_triple() {
		let resp = RegisterWorkerResponse {
			protocol_version: PROTOCOL_VERSION,
			worker_id: 17,
			client_cert_pem: "-----BEGIN CERTIFICATE-----\n...".into(),
			client_key_pem: "-----BEGIN PRIVATE KEY-----\n...".into(),
			ca_cert_pem: "-----BEGIN CERTIFICATE-----\n...".into(),
		};
		let s = serde_json::to_string(&resp).unwrap();
		let back: RegisterWorkerResponse = serde_json::from_str(&s).unwrap();
		assert_eq!(resp, back);
	}

	#[test]
	fn rotate_repo_pat_request_does_not_use_storage_ids() {
		let req = RotateRepoPatRequest {
			protocol_version: PROTOCOL_VERSION,
			github_pat: "ghp_new".into(),
		};
		let s = serde_json::to_string(&req).unwrap();
		assert!(s.contains("github_pat"));
		assert!(!s.contains("pat_secret_id"));
		let back: RotateRepoPatRequest = serde_json::from_str(&s).unwrap();
		assert_eq!(req, back);
	}

	#[test]
	fn set_repo_github_reporting_request_does_not_use_storage_ids() {
		let req = SetRepoGithubReportingRequest {
			protocol_version: PROTOCOL_VERSION,
			target_owner: "acme".into(),
			target_repo: "tracker".into(),
			github_pat: "ghp_new".into(),
		};
		let s = serde_json::to_string(&req).unwrap();
		assert!(s.contains("github_pat"));
		assert!(!s.contains("pat_secret_id"));
		let back: SetRepoGithubReportingRequest = serde_json::from_str(&s).unwrap();
		assert_eq!(req, back);
	}
}
