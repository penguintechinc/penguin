//! Tamper-protection secret hashing and verification.
//!
//! The self-protection subsystem gates a privileged operation (e.g.
//! disabling tamper protection) behind a shared secret. The secret is never
//! stored or logged in plaintext — only its Argon2id PHC-format hash is
//! persisted, and [`verify_secret`] checks a candidate against that hash in
//! constant time via the `argon2` crate's verifier.

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};

use crate::error::SelfProtectError;

/// Hashes `plain` with Argon2id (`Argon2::default()`, a fresh random salt
/// per call) and returns the result as a PHC-format string suitable for
/// storage. The plaintext itself is never returned or logged — only this
/// hash. Fails only if the underlying Argon2 hashing operation itself
/// errors (e.g. an internal parameter/allocation failure), mapped to
/// [`SelfProtectError::Hashing`].
pub fn hash_secret(plain: &str) -> Result<String, SelfProtectError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|_| SelfProtectError::Hashing)?;
    Ok(hash.to_string())
}

/// Checks whether `plain` matches the Argon2id PHC-format hash `phc`
/// (as produced by [`hash_secret`]). Returns `true` iff it verifies, and
/// `false` for any other outcome — a genuine mismatch, or `phc` being
/// malformed/not a valid PHC string — so a caller can treat this as a
/// simple, never-panicking boolean check. Verification is constant-time,
/// provided by the `argon2` crate's `PasswordVerifier` implementation.
pub fn verify_secret(plain: &str, phc: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc) else {
        return false;
    };
    Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_verifies_against_its_hash_and_rejects_wrong() {
        let phc = hash_secret("correct horse").unwrap();
        assert!(verify_secret("correct horse", &phc));
        assert!(!verify_secret("Tr0ub4dor", &phc));
        assert_ne!(
            phc, "correct horse",
            "stored form is a hash, never the plaintext"
        );
    }

    #[test]
    fn verify_secret_rejects_malformed_phc_without_panicking() {
        assert!(!verify_secret("x", "not-a-valid-phc"));
    }
}
