//! Static asset handlers.
//!
//! Assets are compiled in with `include_str!`, which keeps the crate free
//! of a static-file server dependency and means there is no path handling
//! to get wrong. Nothing is templated: no server-side interpolation
//! happens anywhere in this crate, so finding text — which originates in
//! scanned repositories and LLM output, and is therefore untrusted — is
//! never spliced into markup. The browser renders it from JSON instead.

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::state::WebState;

const INDEX_HTML: &str = include_str!("../../assets/index.html");
const APP_CSS: &str = include_str!("../../assets/app.css");
const APP_JS: &str = include_str!("../../assets/app.js");

/// Content Security Policy for the document.
///
/// `default-src 'none'` then allow only what we actually use. No
/// `'unsafe-inline'`: the stylesheet and script are separate files, so
/// injected markup cannot execute even if a rendering bug let some
/// through. `form-action 'none'` because the UI submits via fetch, and
/// `frame-ancestors 'none'` so the dashboard cannot be framed.
const CSP: &str = "default-src 'none'; \
	script-src 'self'; \
	style-src 'self'; \
	connect-src 'self'; \
	img-src 'self'; \
	form-action 'none'; \
	frame-ancestors 'none'; \
	base-uri 'none'";

/// `GET /` — serve the document.
///
/// The capability arrives after `#t=` in the startup URL. Fragments never
/// enter an HTTP request; `app.js` moves it into origin-scoped
/// `sessionStorage` before making the first API call.
pub async fn index() -> Response {
	document(INDEX_HTML)
}

fn document(body: &'static str) -> Response {
	(
		StatusCode::OK,
		[
			(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8")),
			(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP)),
			(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
			(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
			(header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
		],
		body,
	)
		.into_response()
}

/// `GET /app.css`
pub async fn stylesheet() -> Response {
	asset("text/css; charset=utf-8", APP_CSS)
}

/// `GET /app.js`
pub async fn script() -> Response {
	asset("text/javascript; charset=utf-8", APP_JS)
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
	(
		StatusCode::OK,
		[
			(header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
			(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
			(header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
		],
		body,
	)
		.into_response()
}

/// `GET /api/config` — non-secret settings the page needs to boot.
pub async fn client_config(State(state): State<WebState>) -> Response {
	axum::Json(serde_json::json!({
		"poll_seconds": state.poll_seconds,
		"protocol_version": loupe_proto::PROTOCOL_VERSION,
		"request_header": crate::guard::REQUEST_HEADER,
		"capability_header": crate::token::HEADER_NAME,
	}))
	.into_response()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn embedded_assets_are_present() {
		assert!(INDEX_HTML.contains("<!DOCTYPE html>"), "index.html should be a document");
		assert!(!APP_CSS.trim().is_empty());
		assert!(!APP_JS.trim().is_empty());
	}

	#[test]
	fn capability_bootstrap_is_origin_scoped() {
		assert!(APP_JS.contains("window.location.hash"), "the token must arrive in a fragment");
		assert!(APP_JS.contains("sessionStorage"), "the token must be scoped to this origin");
		assert!(!APP_JS.contains("document.cookie"), "cookies are shared across loopback ports");
	}

	#[test]
	fn job_refreshes_cannot_overlap() {
		assert!(
			!APP_JS.contains("window.setInterval"),
			"interval polling can start another refresh while the prior one is running"
		);
		assert!(
			APP_JS.contains("let JOBS_LOAD = null;"),
			"job refreshes need a single-flight guard"
		);
		assert!(APP_JS.contains("while (JOBS_RELOAD_REQUESTED)"));
		assert!(APP_JS.contains("await loadJobsNow()"));
	}

	#[test]
	fn the_document_has_no_inline_script_or_style() {
		// The CSP forbids both; catch a regression at build time rather
		// than as a blank page in the browser.
		assert!(!INDEX_HTML.contains("<script>"), "inline script violates the CSP");
		assert!(!INDEX_HTML.contains("<style>"), "inline style violates the CSP");
		assert!(!INDEX_HTML.contains("onclick="), "inline handler violates the CSP");
		assert!(!INDEX_HTML.contains("onload="), "inline handler violates the CSP");
	}

	#[test]
	fn the_script_never_assigns_inner_html() {
		// Finding text is attacker-influenced. Rendering it through
		// innerHTML would be an XSS sink, so the whole UI is built with
		// textContent / createElement instead.
		assert!(
			!APP_JS.contains("innerHTML"),
			"app.js must not use innerHTML: finding text is untrusted"
		);
		assert!(!APP_JS.contains("outerHTML"), "app.js must not use outerHTML");
		assert!(!APP_JS.contains("insertAdjacentHTML"));
		assert!(!APP_JS.contains("document.write"));
		assert!(!APP_JS.contains("eval("));
	}

	/// The page pre-checks a search term so it can say "no usable terms"
	/// instead of the misleading "no matches" the server's empty result
	/// would imply. That only works while it strips the same characters as
	/// `loupe_storage::findings::sanitize_fts_query`, which drops tokens
	/// under two characters and removes `" * : ( ) '`. Pin the character
	/// class so a change on either side is noticed here.
	#[test]
	fn the_search_prefilter_matches_the_server_sanitizer() {
		assert!(
			APP_JS.contains(r#"replace(/["*:()']/g, "")"#),
			"app.js must strip exactly the characters sanitize_fts_query does"
		);
		assert!(
			APP_JS.contains("token.length >= 2"),
			"app.js must drop tokens shorter than 2 characters, as the server does"
		);
	}

	#[test]
	fn csp_denies_by_default_and_allows_no_inline() {
		assert!(CSP.starts_with("default-src 'none'"));
		assert!(!CSP.contains("unsafe-inline"));
		assert!(!CSP.contains("unsafe-eval"));
		assert!(CSP.contains("frame-ancestors 'none'"));
		assert!(CSP.contains("form-action 'none'"));
	}
}
