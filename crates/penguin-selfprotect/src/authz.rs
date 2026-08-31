//! Tamper-protection secret hashing/verification, and the teardown
//! authorization decision built on top of it.
//!
//! The self-protection subsystem gates a privileged operation (e.g.
//! disabling tamper protection) behind a shared secret. The secret is never
//! stored or logged in plaintext — only its Argon2id PHC-format hash is
//! persisted, and [`verify_secret`] checks a candidate against that hash in
//! constant time via the `argon2` crate's verifier.
//!
//! [`authorize`] is the crux of the uninstall/teardown gate: it decides
//! whether a teardown request is allowed, and by which of three paths
//! ([`TeardownAuthz`]). Two of those paths — a console-recorded
//! deauthorization and a signed break-glass token — are deliberately
//! unconditional on local state, so a lost local secret or an unreachable
//! console can never turn into a permanent lockout for a legitimate admin.

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

/// The outcome of [`authorize`]: which path (if any) authorized a
/// teardown/uninstall request. Variant order carries no meaning by itself —
/// [`authorize`]'s internal precedence is what determines which variant a
/// given `(TeardownInput, TeardownCtx)` pair produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownAuthz {
    /// The controller/console already recorded this node as deauthorized
    /// (removal approved centrally). This wins over every other path, even
    /// with no local secret or break-glass token presented at all — an
    /// admin who deauthorized a node from the console can always complete
    /// the local teardown.
    NodeDeauthorized,
    /// The caller is root and presented the correct local tamper-protection
    /// secret, verified against [`TeardownCtx::secret_phc`].
    LocalSecret,
    /// The caller presented a valid, node-bound break-glass token: a
    /// minisign signature over this node's ID, issued out of band for
    /// emergency recovery when the local secret is unknown or lost and the
    /// console is unreachable. Unconditional on local state, so a lost
    /// secret can never be a permanent lockout either.
    BreakGlassToken,
    /// None of the above authorized the request.
    Unauthorized,
}

/// Caller-supplied credentials for a teardown/uninstall request. At most
/// one of `secret` or `break_glass` need be present — either may authorize
/// the request via [`authorize`], depending on [`TeardownCtx`].
///
/// Deliberately does not derive `Debug` — see the manual `impl` below.
#[derive(Clone)]
pub struct TeardownInput {
    /// Candidate plaintext for the local tamper-protection secret, if the
    /// caller supplied one.
    pub secret: Option<String>,
    /// Candidate break-glass token (minisign signature text), if the
    /// caller supplied one.
    pub break_glass: Option<String>,
}

impl std::fmt::Debug for TeardownInput {
    /// Redacts both fields rather than deriving `Debug`: `secret` is a
    /// live plaintext credential and `break_glass` is a live recovery
    /// token, so a derived `Debug` would let a future accidental
    /// `tracing::debug!("{:?}", input)` (or similar) leak either straight
    /// into logs. Prints `<redacted>` when a field is present, `None` when
    /// it is absent — enough to see *that* credentials were supplied
    /// without ever printing their value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn presence(value: &Option<String>) -> &'static str {
            if value.is_some() {
                "<redacted>"
            } else {
                "None"
            }
        }
        write!(
            f,
            "TeardownInput {{ secret: {}, break_glass: {} }}",
            presence(&self.secret),
            presence(&self.break_glass)
        )
    }
}

/// Everything [`authorize`] needs to know about this node's current state
/// to evaluate a [`TeardownInput`]: the stored local-secret hash, this
/// node's identity and trusted signing key for break-glass verification,
/// whether the caller is root, and whether the console has already
/// deauthorized this node.
#[derive(Debug, Clone)]
pub struct TeardownCtx {
    /// Whether the process attempting teardown is running as root. Gates
    /// only the local-secret path — the OS itself still lets root remove
    /// files directly, but this service's own uninstall verb refuses a
    /// non-root caller regardless of secret correctness.
    pub is_root: bool,
    /// The stored Argon2id PHC hash of the local tamper-protection secret
    /// (as produced by [`hash_secret`]), if one has been configured. `None`
    /// means the local-secret path can never be satisfied.
    pub secret_phc: Option<String>,
    /// This node's identity, as embedded in a valid break-glass token.
    pub node_id: String,
    /// Minisign public key text trusted to sign break-glass tokens.
    pub pubkey: String,
    /// Whether the controller/console has already recorded this node as
    /// deauthorized (removal approved centrally).
    pub console_deauthorized: bool,
}

/// Checks whether `token` is a valid minisign signature over `node_id`'s
/// own UTF-8 bytes, verified against `pubkey_text`. This makes a
/// break-glass token node-bound: a token issued for a different node's ID
/// never verifies here, even against the same trusted signing key. Returns
/// `false` on any failure — malformed token, wrong key, wrong node — and
/// never panics.
pub fn verify_break_glass(token: &str, node_id: &str, pubkey_text: &str) -> bool {
    penguin_update::verify(node_id.as_bytes(), token, pubkey_text).is_ok()
}

/// Decides whether a teardown/uninstall request is authorized, and by
/// which path. Precedence (first match wins — order is the security-
/// relevant part of this function):
///
/// 1. `ctx.console_deauthorized` → [`TeardownAuthz::NodeDeauthorized`].
///    The console already approved removal, so this overrides everything
///    else, even with no local credentials presented at all.
/// 2. A `break_glass` token that verifies for `ctx.node_id`/`ctx.pubkey` →
///    [`TeardownAuthz::BreakGlassToken`]. The guaranteed emergency path,
///    independent of whether a local secret is configured or the console
///    is reachable.
/// 3. A root caller presenting the `secret` that matches `ctx.secret_phc`
///    → [`TeardownAuthz::LocalSecret`]. `ctx.is_root` gates only this
///    path.
/// 4. Otherwise → [`TeardownAuthz::Unauthorized`].
///
/// [`TeardownAuthz::NodeDeauthorized`] and [`TeardownAuthz::BreakGlassToken`]
/// are deliberately unconditional on local state — they are the two
/// guaranteed overrides that keep a lost local secret, or an admin who can
/// no longer authenticate locally, from becoming a permanent teardown
/// lockout. Never panics.
pub fn authorize(input: &TeardownInput, ctx: &TeardownCtx) -> TeardownAuthz {
    if ctx.console_deauthorized {
        return TeardownAuthz::NodeDeauthorized;
    }

    let break_glass_ok = input
        .break_glass
        .as_deref()
        .is_some_and(|token| verify_break_glass(token, &ctx.node_id, &ctx.pubkey));
    if break_glass_ok {
        return TeardownAuthz::BreakGlassToken;
    }

    let local_secret_ok = ctx.is_root
        && input
            .secret
            .as_deref()
            .zip(ctx.secret_phc.as_deref())
            .is_some_and(|(secret, phc)| verify_secret(secret, phc));
    if local_secret_ok {
        return TeardownAuthz::LocalSecret;
    }

    TeardownAuthz::Unauthorized
}

/// Test-only fixture: a shared throwaway minisign keypair for signing
/// break-glass tokens, mirroring `manifest.rs`'s own `testfix` module (see
/// that module's doc comment for why `minisign`, the signing-capable
/// crate, is a dev-dependency only). The keypair is lazily generated once
/// per test binary and reused so [`pubkey`](testfix::pubkey) and
/// [`sign_break_glass`](testfix::sign_break_glass) always agree on the
/// signing key.
#[cfg(test)]
mod testfix {
    use std::io::Cursor;
    use std::sync::OnceLock;

    static KEYPAIR: OnceLock<minisign::KeyPair> = OnceLock::new();

    fn keypair() -> &'static minisign::KeyPair {
        KEYPAIR.get_or_init(|| {
            minisign::KeyPair::generate_unencrypted_keypair().expect("generate minisign keypair")
        })
    }

    /// The minisign public key text for the shared test keypair — pass as
    /// `TeardownCtx::pubkey` in tests that also call
    /// [`sign_break_glass`](testfix::sign_break_glass).
    pub(crate) fn pubkey() -> String {
        keypair().pk.to_box().expect("public key box").into_string()
    }

    /// Signs `node_id` with the shared test keypair, returning a
    /// break-glass token that [`super::verify_break_glass`] accepts for
    /// that exact node ID and [`pubkey`](testfix::pubkey).
    pub(crate) fn sign_break_glass(node_id: &str) -> String {
        let keypair = keypair();
        let signature_box = minisign::sign(
            Some(&keypair.pk),
            &keypair.sk,
            Cursor::new(node_id.as_bytes()),
            None,
            None,
        )
        .expect("sign break-glass token");
        signature_box.into_string()
    }
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

    #[test]
    fn authorize_accepts_each_valid_path_and_refuses_otherwise() {
        let ctx = TeardownCtx {
            is_root: true,
            secret_phc: Some(hash_secret("s3cret").unwrap()),
            node_id: "n-1".into(),
            pubkey: testfix::pubkey(),
            console_deauthorized: false,
        };
        // wrong/no creds while armed → Unauthorized
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: None,
                    break_glass: None
                },
                &ctx
            ),
            TeardownAuthz::Unauthorized
        );
        // correct local secret → LocalSecret
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: Some("s3cret".into()),
                    break_glass: None
                },
                &ctx
            ),
            TeardownAuthz::LocalSecret
        );
        // valid break-glass token → BreakGlassToken
        let tok = testfix::sign_break_glass("n-1");
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: None,
                    break_glass: Some(tok)
                },
                &ctx
            ),
            TeardownAuthz::BreakGlassToken
        );
        // console said remove → NodeDeauthorized even with no local creds
        let mut ctx2 = ctx.clone();
        ctx2.console_deauthorized = true;
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: None,
                    break_glass: None
                },
                &ctx2
            ),
            TeardownAuthz::NodeDeauthorized
        );
    }

    #[test]
    fn break_glass_token_is_bound_to_the_node_it_was_signed_for() {
        let ctx = TeardownCtx {
            is_root: true,
            secret_phc: Some(hash_secret("s3cret").unwrap()),
            node_id: "n-1".into(),
            pubkey: testfix::pubkey(),
            console_deauthorized: false,
        };
        // token signed for a different node id must not authorize this node
        let tok = testfix::sign_break_glass("n-2");
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: None,
                    break_glass: Some(tok)
                },
                &ctx
            ),
            TeardownAuthz::Unauthorized
        );
    }

    #[test]
    fn teardown_input_debug_redacts_secret_and_break_glass_when_present() {
        let input = TeardownInput {
            secret: Some("s3cret".to_string()),
            break_glass: Some("break-glass-token-xyz".to_string()),
        };
        let debug_str = format!("{input:?}");
        assert!(
            !debug_str.contains("s3cret"),
            "Debug output must not leak the secret: {debug_str}"
        );
        assert!(
            !debug_str.contains("break-glass-token-xyz"),
            "Debug output must not leak the break-glass token: {debug_str}"
        );
        assert!(debug_str.contains("<redacted>"));
    }

    #[test]
    fn teardown_input_debug_shows_none_when_absent() {
        let input = TeardownInput {
            secret: None,
            break_glass: None,
        };
        let debug_str = format!("{input:?}");
        assert!(!debug_str.contains("<redacted>"));
        assert_eq!(
            debug_str,
            "TeardownInput { secret: None, break_glass: None }"
        );
    }

    #[test]
    fn non_root_caller_cannot_use_local_secret_path() {
        let ctx = TeardownCtx {
            is_root: false,
            secret_phc: Some(hash_secret("s3cret").unwrap()),
            node_id: "n-1".into(),
            pubkey: testfix::pubkey(),
            console_deauthorized: false,
        };
        assert_eq!(
            authorize(
                &TeardownInput {
                    secret: Some("s3cret".into()),
                    break_glass: None
                },
                &ctx
            ),
            TeardownAuthz::Unauthorized
        );
    }
}
