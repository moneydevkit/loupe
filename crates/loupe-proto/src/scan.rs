use loupe_core::{JobKind, JobState};
use serde::{Deserialize, Serialize};

/// Body of `POST /v1/repos/:id/scan` (admin).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRequest {
	pub protocol_version: u16,
	#[serde(default)]
	pub incremental: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanResponse {
	pub protocol_version: u16,
	pub job_id: i64,
}

/// Listing entry for `GET /v1/jobs` and `GET /v1/jobs/:id`.
///
/// The lease/timing/error block below is additive: every field is
/// `Option` with `skip_serializing_if`, so a client built against this
/// protocol still reads responses from a server that predates them. They
/// exist so an operator surface can answer "how long did this take, who
/// ran it, and why did it fail" without a second round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfo {
	pub job_id: i64,
	pub repo_id: i64,
	pub kind: JobKind,
	pub state: JobState,
	pub incremental: bool,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub since_sha: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub head_sha: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub parent_job_id: Option<i64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub target_finding_id: Option<i64>,
	pub attempts: u32,
	pub enqueued_at: i64,
	/// Worker that holds the lease, or last held it. `None` while the job
	/// has never been leased; the reaper and a retry both clear it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub worker_id: Option<i64>,
	/// When the current lease lapses. Only meaningful while `state` is
	/// `leased` — a countdown against it is how a UI spots a stalled
	/// worker before the reaper gets to it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lease_expires_at: Option<i64>,
	/// When the job was first leased. Paired with `finished_at` this
	/// gives the run duration.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub started_at: Option<i64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub finished_at: Option<i64>,
	/// Why a `failed` or `cancelled` job ended that way.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
}

#[cfg(test)]
mod tests {
	use loupe_core::{JobKind, JobState};

	use super::*;

	#[test]
	fn scan_request_defaults_incremental_false() {
		let req: ScanRequest = serde_json::from_str(r#"{"protocol_version":1}"#).unwrap();
		assert!(!req.incremental);
	}

	fn queued_job() -> JobInfo {
		JobInfo {
			job_id: 1,
			repo_id: 2,
			kind: JobKind::Scan,
			state: JobState::Queued,
			incremental: false,
			since_sha: None,
			head_sha: None,
			parent_job_id: None,
			target_finding_id: None,
			attempts: 0,
			enqueued_at: 1_700_000_000,
			worker_id: None,
			lease_expires_at: None,
			started_at: None,
			finished_at: None,
			error: None,
		}
	}

	#[test]
	fn job_info_round_trips() {
		let info = queued_job();
		let s = serde_json::to_string(&info).unwrap();
		let back: JobInfo = serde_json::from_str(&s).unwrap();
		assert_eq!(info, back);
	}

	#[test]
	fn verify_job_carries_parentage() {
		let info = JobInfo {
			job_id: 5,
			kind: JobKind::Verify,
			state: JobState::Leased,
			head_sha: Some("abc123".into()),
			parent_job_id: Some(1),
			target_finding_id: Some(42),
			enqueued_at: 1_700_000_100,
			..queued_job()
		};
		let s = serde_json::to_string(&info).unwrap();
		let back: JobInfo = serde_json::from_str(&s).unwrap();
		assert_eq!(info, back);
	}

	#[test]
	fn job_info_round_trips_lease_and_failure_detail() {
		let info = JobInfo {
			state: JobState::Failed,
			attempts: 3,
			worker_id: Some(7),
			lease_expires_at: Some(1_700_000_600),
			started_at: Some(1_700_000_010),
			finished_at: Some(1_700_000_400),
			error: Some("lease expired after max attempts".into()),
			..queued_job()
		};
		let s = serde_json::to_string(&info).unwrap();
		let back: JobInfo = serde_json::from_str(&s).unwrap();
		assert_eq!(info, back);
		assert_eq!(
			back.finished_at.unwrap() - back.started_at.unwrap(),
			390,
			"duration is derivable"
		);
	}

	#[test]
	fn job_info_reads_a_response_without_lease_detail() {
		// The lease/timing block is additive, so a payload from a server
		// that predates it must still deserialize.
		let s = r#"{"job_id":1,"repo_id":2,"kind":"scan","state":"queued",
			"incremental":false,"attempts":0,"enqueued_at":1700000000}"#;
		let info: JobInfo = serde_json::from_str(s).unwrap();
		assert_eq!(info, queued_job());
	}

	#[test]
	fn queued_job_omits_the_empty_lease_block() {
		let s = serde_json::to_string(&queued_job()).unwrap();
		for absent in ["worker_id", "lease_expires_at", "started_at", "finished_at", "error"] {
			assert!(!s.contains(absent), "{absent} should be omitted when unset: {s}");
		}
	}
}
