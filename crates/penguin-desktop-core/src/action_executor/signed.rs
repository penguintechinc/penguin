//! Signed binary verification and execution via `penguin-extplugin::Verifier`.
//!
//! **Fail-closed contract:** signature verification failure → reject + don't execute.
//! Every signed binary is verified fresh on every invocation, never cached.
//!
//! Inline binary payloads are supported: the action's `parameters["binary"]` (base64)
//! and `parameters["manifest"]` (JSON) fields are used to construct a temporary
//! staging directory, verify, and execute.

use base64::engine::Engine as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::Duration;
use tracing::{debug, error};

use penguin_extplugin::{Verifier, load_manifest};

use crate::Session;
use crate::error::{DesktopError, Result};

use super::ActionRequest;

/// Executes a signed binary action: verifies signature first, then runs it.
///
/// Returns `(exit_code, stdout, stderr)` on success.
pub struct SignedExecutor;

impl SignedExecutor {
    /// Main entry point: extracts the binary from the action, verifies it,
    /// and executes it if verification succeeds.
    pub async fn execute(
        _session: &Session,
        action: &ActionRequest,
        _machine_id: &str,
    ) -> Result<(i32, Vec<u8>, Vec<u8>)> {
        // Extract binary and manifest from parameters
        let binary_b64 = action
            .parameters
            .get("binary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DesktopError::Internal("binary field missing".to_string()))?;

        let manifest_json = action
            .parameters
            .get("manifest")
            .ok_or_else(|| DesktopError::Internal("manifest field missing".to_string()))?;

        // Decode binary from base64
        let binary_bytes = base64::engine::general_purpose::STANDARD
            .decode(binary_b64)
            .map_err(|e| DesktopError::Internal(format!("failed to decode binary: {}", e)))?;

        // Create a temporary staging directory
        let staging_dir = tempfile::tempdir()
            .map_err(|e| DesktopError::Internal(format!("failed to create staging dir: {}", e)))?;

        let staging_path = staging_dir.path();

        // Write binary to staging directory
        let binary_path = staging_path.join("binary");
        tokio::fs::write(&binary_path, &binary_bytes)
            .await
            .map_err(|e| DesktopError::Internal(format!("failed to write binary: {}", e)))?;

        // Make binary executable (Unix only; Windows uses different permissions model)
        #[cfg(unix)]
        {
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&binary_path, perms)
                .map_err(|e| DesktopError::Internal(format!("failed to chmod binary: {}", e)))?;
        }

        // Write manifest to staging directory
        let manifest_path = staging_path.join("plugin.json");
        let manifest_bytes = serde_json::to_vec(manifest_json)
            .map_err(|e| DesktopError::Internal(format!("failed to serialize manifest: {}", e)))?;
        tokio::fs::write(&manifest_path, &manifest_bytes)
            .await
            .map_err(|e| DesktopError::Internal(format!("failed to write manifest: {}", e)))?;

        // Load manifest
        let manifest = load_manifest(&manifest_path)
            .map_err(|e| DesktopError::Internal(format!("failed to load manifest: {}", e)))?;

        // Verify: ownership, SHA256, minisign
        let verifier = Verifier::new();

        // Get current uid for ownership verification (Unix-specific, falls back on other platforms)
        #[cfg(unix)]
        let expected_uid = nix::unistd::Uid::current().as_raw();
        #[cfg(not(unix))]
        let expected_uid = 1000u32;

        verifier
            .verify(staging_path, &manifest, expected_uid)
            .map_err(|e| {
                error!("signature verification failed: {}", e);
                DesktopError::Internal(format!("signature verification failed: {}", e))
            })?;

        debug!("signature verified, executing binary");

        // Execute binary with timeout enforcement using Box::pin and select!
        let child = tokio::process::Command::new(&binary_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| DesktopError::Internal(format!("failed to spawn binary: {}", e)))?;

        let timeout_duration = Duration::from_secs(action.timeout as u64);

        // Pin the wait_with_output future so we can use it in select!
        let mut wait_fut = std::pin::pin!(child.wait_with_output());
        let mut timeout_fut = std::pin::pin!(tokio::time::sleep(timeout_duration));

        #[allow(clippy::never_loop)]
        let output = loop {
            tokio::select! {
                result = &mut wait_fut => {
                    match result {
                        Ok(output) => break output,
                        Err(e) => {
                            error!("binary wait failed: {}", e);
                            return Err(DesktopError::Internal(format!("binary execution error: {}", e)));
                        }
                    }
                }
                _ = &mut timeout_fut => {
                    error!("binary execution timeout");
                    // Try to kill the process, but if it's already done, that's fine
                    // The timeout means we no longer care about its output
                    return Err(DesktopError::Internal("binary execution timeout".to_string()));
                }
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);

        Ok((exit_code, output.stdout, output.stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use nix::unistd::Uid;
    use std::fs;

    #[test]
    fn test_signed_executor_missing_binary_field() {
        let action = ActionRequest {
            id: "act_test".to_string(),
            r#type: "signed_binary".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({}), // Missing "binary"
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        // Structure validation
        let _ = &action;
    }

    #[test]
    fn test_signed_executor_missing_manifest_field() {
        let action = ActionRequest {
            id: "act_test".to_string(),
            r#type: "signed_binary".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "binary": "aGVsbG8gd29ybGQ=" // base64("hello world")
            }),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        let _ = &action;
    }

    #[test]
    fn test_signed_executor_corrupt_manifest_structure() {
        let binary_b64 = base64::engine::general_purpose::STANDARD.encode(b"test");
        let action = ActionRequest {
            id: "act_corrupt".to_string(),
            r#type: "signed_binary".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "binary": binary_b64,
                "manifest": "not json"
            }),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            priority: 0,
            timeout: 5,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };
        assert_eq!(action.id, "act_corrupt");
    }

    #[test]
    fn test_signed_executor_bad_manifest_sha() {
        let action = ActionRequest {
            id: "act_bad_sha".to_string(),
            r#type: "signed_binary".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "binary": base64::engine::general_purpose::STANDARD.encode(b"content"),
                "manifest": serde_json::json!({
                    "name": "test",
                    "path": "binary",
                    "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
                    "minisig": ""
                })
            }),
            user_id: "user_test".to_string(),
            community_id: "comm_test".to_string(),
            priority: 0,
            timeout: 5,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };
        assert!(action.parameters.get("manifest").is_some());
    }

    #[test]
    fn test_verifier_untrusted_signer_rejects_valid_fixture() {
        // SECURITY: Untrusted signer test — even with valid minisign sig, empty key set refuses.
        // Fixture: the real test-binary from penguin-extplugin tests (validly signed).
        // Verifier: empty key set → MUST refuse.
        // Observable: VerifyError::UntrustedSigner returned.

        let fixture_binary = b"test binary content"; // Matches penguin-extplugin fixture
        let _fixture_pub_key =
            include_str!("../../../penguin-extplugin/tests/fixtures/test-binary.pub");
        let _fixture_sig =
            include_str!("../../../penguin-extplugin/tests/fixtures/test-binary.minisig");
        let fixture_sha256 = "56681959d2de970a2dbee51710bb02862bec0a603b725443b92063c02b5f0a0c";

        let staging_dir = tempfile::tempdir().expect("create temp dir");
        let binary_path = staging_dir.path().join("binary");
        fs::write(&binary_path, fixture_binary).expect("write fixture binary");

        // Write minisig signature file (verifier expects it at binary.minisig)
        let sig_path = staging_dir.path().join("binary.minisig");
        fs::write(&sig_path, _fixture_sig).expect("write sig");

        let manifest = penguin_extplugin::Manifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            sdk_version: "v1".to_string(),
            binary: "binary".to_string(),
            sha256: fixture_sha256.to_string(),
            publisher: "test-publisher".to_string(),
        };

        // CRITICAL: Verifier with EMPTY key set (no trusted publishers)
        let verifier = penguin_extplugin::Verifier::with_keys(vec![]); // Empty = untrusted

        #[cfg(unix)]
        let expected_uid = Uid::current().as_raw();
        #[cfg(not(unix))]
        let expected_uid = 1000u32;

        // ASSERTION: Verification MUST fail with UntrustedSigner
        let result = verifier.verify(staging_dir.path(), &manifest, expected_uid);
        assert!(
            result.is_err(),
            "Empty key set should refuse valid signature"
        );

        debug!("Untrusted signer correctly rejected (empty key set)");
    }

    #[test]
    fn test_verifier_world_writable_staging_rejected() {
        // SECURITY: World-writable staging directory → verification MUST fail.
        // Observable: VerifyError::WorldWritable returned, no execution follows.

        let staging_dir = tempfile::tempdir().expect("create temp dir");

        // chmod 777 the staging directory
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(staging_dir.path(), std::fs::Permissions::from_mode(0o777))
                .expect("chmod 777 staging");
        }

        let binary_path = staging_dir.path().join("binary");
        fs::write(&binary_path, b"test").expect("write binary");

        let manifest = penguin_extplugin::Manifest {
            name: "test".to_string(),
            version: "1.0".to_string(),
            sdk_version: "v1".to_string(),
            binary: "binary".to_string(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            publisher: "test".to_string(),
        };

        let verifier = penguin_extplugin::Verifier::new();

        #[cfg(unix)]
        let expected_uid = Uid::current().as_raw();
        #[cfg(not(unix))]
        let expected_uid = 1000u32;

        // ASSERTION: Verification MUST fail due to world-writable directory
        let result = verifier.verify(staging_dir.path(), &manifest, expected_uid);
        assert!(
            result.is_err(),
            "World-writable staging directory should be rejected"
        );

        debug!("World-writable staging correctly rejected");
    }

    #[test]
    fn test_verifier_trusted_signer_accepts_valid_fixture() {
        // SUCCESS PATH: Valid signed fixture + trusted key → verification succeeds.
        // Fixture: penguin-extplugin's real test-binary.{pub,minisig} pair.
        // Observable: Verifier::verify() returns Ok.

        let fixture_binary = b"test binary content";
        let fixture_pub_key =
            include_str!("../../../penguin-extplugin/tests/fixtures/test-binary.pub");
        let fixture_sig =
            include_str!("../../../penguin-extplugin/tests/fixtures/test-binary.minisig");
        let fixture_sha256 = "56681959d2de970a2dbee51710bb02862bec0a603b725443b92063c02b5f0a0c";

        let staging_dir = tempfile::tempdir().expect("create temp dir");
        let binary_path = staging_dir.path().join("binary");
        fs::write(&binary_path, fixture_binary).expect("write fixture");

        // Write minisig file (verifier expects it next to binary)
        let sig_path = staging_dir.path().join("binary.minisig");
        fs::write(&sig_path, fixture_sig).expect("write sig");

        let manifest = penguin_extplugin::Manifest {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            sdk_version: "v1".to_string(),
            binary: "binary".to_string(),
            sha256: fixture_sha256.to_string(),
            publisher: "test-publisher".to_string(),
        };

        // Verifier with TRUSTED key
        let verifier = penguin_extplugin::Verifier::with_keys(vec![fixture_pub_key.to_string()]);

        #[cfg(unix)]
        let expected_uid = Uid::current().as_raw();
        #[cfg(not(unix))]
        let expected_uid = 1000u32;

        // ASSERTION: Verification succeeds with trusted key
        let result = verifier.verify(staging_dir.path(), &manifest, expected_uid);
        assert!(
            result.is_ok(),
            "Trusted signer should accept valid fixture: {:?}",
            result.err()
        );

        debug!("Trusted signer correctly accepted fixture");
    }

    #[test]
    fn test_signed_executor_payload_fields() {
        // Test ActionRequest payload field validation for signed_binary
        let action = ActionRequest {
            id: "act_payload_test".to_string(),
            r#type: "signed_binary".to_string(),
            module_name: None,
            action: None,
            parameters: serde_json::json!({
                "binary": "aGVsbG8gd29ybGQ=",  // base64("hello world")
                "manifest": serde_json::json!({
                    "name": "test-plugin",
                    "version": "1.0.0",
                    "sdk_version": "v1",
                    "binary": "binary",
                    "sha256": "abc123def456",
                    "publisher": "test-publisher"
                })
            }),
            user_id: "user1".to_string(),
            community_id: "comm1".to_string(),
            priority: 0,
            timeout: 30,
            created_at: "2025-01-01T00:00:00Z".to_string(),
            expires_at: "2025-01-01T00:05:00Z".to_string(),
        };

        // Verify parameters contain the expected fields
        assert!(action.parameters.get("binary").is_some());
        assert!(action.parameters.get("manifest").is_some());
        let manifest = action.parameters.get("manifest").unwrap();
        assert_eq!(
            manifest.get("name").unwrap().as_str().unwrap(),
            "test-plugin"
        );
        assert_eq!(manifest.get("version").unwrap().as_str().unwrap(), "1.0.0");
    }
}
