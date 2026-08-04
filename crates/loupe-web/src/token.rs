//! The dashboard's local capability token.
//!
//! Loopback binding alone only proves a caller is on this machine — any
//! local uid can open a TCP connection, including one that cannot read
//! the admin key the daemon holds. The token closes that gap: it is
//! printed once to the operator's terminal at startup, so another user
//! on a shared host cannot obtain it.
//!
//! This is deliberately *not* a second authentication scheme for loupe.
//! It grants nothing upstream and is meaningless to `loupe-server`; it
//! only gates access to this process, which is what holds the admin
//! credential.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand_core::{OsRng, RngCore};

/// Header carrying the origin-scoped capability on API requests.
pub const HEADER_NAME: &str = "X-Loupe-Capability";

/// Length of the base64url encoding of 32 bytes with no padding.
const ENCODED_LEN: usize = 43;

/// A capability token. The plaintext exists only to be printed once and
/// handed back by the browser; what we retain is its hash.
#[derive(Clone)]
pub struct Token {
	secret: String,
	hash: [u8; 32],
}

impl Token {
	/// Mint a fresh token from the OS CSPRNG. Mirrors the server's
	/// per-lease capability generation.
	pub fn generate() -> Self {
		let mut random = [0u8; 32];
		OsRng.fill_bytes(&mut random);
		Self::from_secret(URL_SAFE_NO_PAD.encode(random))
	}

	fn from_secret(secret: String) -> Self {
		let hash = *blake3::hash(secret.as_bytes()).as_bytes();
		Self { secret, hash }
	}

	/// The plaintext, for printing the startup URL. Deliberately the only
	/// way to read it, so a stray `Debug`/`Display` can't log it.
	pub fn reveal(&self) -> &str {
		&self.secret
	}

	/// Whether `candidate` is this token.
	///
	/// Compares blake3 hashes rather than the secrets. A timing signal on
	/// hash bytes is useless to an attacker: matching a prefix requires
	/// finding a preimage, not guessing the token.
	pub fn matches(&self, candidate: &str) -> bool {
		if candidate.len() != ENCODED_LEN {
			return false;
		}
		*blake3::hash(candidate.as_bytes()).as_bytes() == self.hash
	}
}

/// Redacted on purpose: a token in a log or a panic message defeats the
/// point of only ever printing it once.
impl std::fmt::Debug for Token {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("Token(redacted)")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn generated_tokens_are_43_chars_and_distinct() {
		let a = Token::generate();
		let b = Token::generate();
		assert_eq!(a.reveal().len(), ENCODED_LEN);
		assert_ne!(a.reveal(), b.reveal(), "each run must mint a fresh token");
	}

	#[test]
	fn token_matches_only_itself() {
		let token = Token::generate();
		assert!(token.matches(token.reveal()));
		assert!(!token.matches(Token::generate().reveal()));
		assert!(!token.matches(""), "empty string must not match");
		assert!(!token.matches("short"), "wrong length must not match");
	}

	#[test]
	fn a_correct_prefix_does_not_match() {
		let token = Token::generate();
		let mut near_miss = token.reveal().to_owned();
		// Flip the last character; everything before it is correct.
		let last = near_miss.pop().unwrap();
		near_miss.push(if last == 'A' { 'B' } else { 'A' });
		assert_eq!(near_miss.len(), ENCODED_LEN);
		assert!(!token.matches(&near_miss));
	}

	#[test]
	fn debug_does_not_reveal_the_secret() {
		let token = Token::generate();
		let rendered = format!("{token:?}");
		assert_eq!(rendered, "Token(redacted)");
		assert!(!rendered.contains(token.reveal()));
	}

	#[test]
	fn capability_header_is_custom_and_namespaced() {
		assert_eq!(HEADER_NAME, "X-Loupe-Capability");
	}
}
