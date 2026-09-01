//! [`SelfProtectError`]: every way loading and verifying an
//! [`crate::IntegrityManifest`] can fail.

/// Every failure mode of loading and verifying an [`crate::IntegrityManifest`].
#[derive(Debug, thiserror::Error)]
pub enum SelfProtectError {
    /// The manifest's signature does not verify against its canonical
    /// bytes with the given public key — the manifest may be tampered,
    /// corrupted in transit, or simply was not signed by the expected key.
    /// Does not carry the underlying `penguin_update::VerifyError`: a
    /// caller deciding whether to trust a manifest only ever needs
    /// "verified or not", never which of minisign's several failure modes
    /// fired.
    #[error("integrity manifest signature verification failed")]
    Signature,
    /// The manifest file could not be read from disk (missing, permission
    /// denied, ...).
    #[error("failed to read integrity manifest: {0}")]
    Io(#[source] std::io::Error),
    /// The manifest bytes were not valid JSON, or did not match the
    /// expected [`crate::IntegrityManifest`] shape.
    #[error("failed to parse integrity manifest: {0}")]
    Parse(#[source] serde_json::Error),
    /// Argon2id hashing of a tamper-protection secret failed (an internal
    /// `argon2` parameter/allocation error, not a verification mismatch —
    /// see [`crate::verify_secret`] for the separate never-fails boolean
    /// check). Does not carry the underlying `argon2::password_hash::Error`
    /// or the plaintext that was being hashed.
    #[error("failed to hash tamper-protection secret")]
    Hashing,
    /// [`crate::heal`] copied a file from its protected copy, but the
    /// bytes that landed at the target path do not match the manifest's
    /// expected SHA-256 hash — the protected copy itself may be
    /// poisoned/tampered, or the write was corrupted. `heal` refuses to
    /// report success in this case, rather than silently trusting an
    /// unverified restore. Does not carry either hash: a caller deciding
    /// whether the restore succeeded only ever needs "verified or not".
    #[error("restored file failed post-heal integrity verification against the manifest")]
    HealVerificationFailed,
}
