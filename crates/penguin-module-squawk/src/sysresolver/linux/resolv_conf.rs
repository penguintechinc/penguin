//! The `/etc/resolv.conf` backend: the universal Linux fallback used when
//! systemd-resolved isn't running. Byte-exact backup/restore preserves
//! `search`/`options`/comments that a bare server-list restore would lose.
//!
//! # Two backups, one precedence
//!
//! Every `apply()` produces *two* independent backups of "what DNS was
//! before": the crash marker's bare `previous_servers` list (owned by
//! [`crate::sysresolver`], shared by every backend) and this backend's own
//! byte-exact copy of the whole file at `<data_dir>/resolv.conf.backup`
//! (owned entirely here). Go had the same two mechanisms but never
//! documented which one wins when both exist. The rule here is explicit:
//!
//! **The byte-exact file backup is authoritative whenever it exists.**
//! [`ResolvConfBackend::restore`] only falls back to reconstructing a
//! bare-server-list file (lossy: no `search`/`options`/comments) from the
//! marker's `fallback_servers` when the byte backup is missing — e.g. the
//! data directory was wiped between apply and restore, or the original
//! `resolv.conf` didn't exist at all (nothing to have byte-backed-up).

use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tracing::warn;

use crate::sysresolver::backend::PlatformBackend;
use crate::sysresolver::error::SysResolverError;
use crate::sysresolver::fsutil::write_owner_only;

/// Identifier persisted in the crash marker for this backend.
pub const BACKEND_NAME: &str = "resolv.conf";

/// Production path for the live system file. Tests point
/// [`ResolvConfBackend::new`] at a tempdir file instead — the same
/// injectable-path pattern Go used for its package-level `resolvConfPath`
/// var — so nothing here ever opens the real `/etc/resolv.conf`.
pub const DEFAULT_RESOLV_CONF_PATH: &str = "/etc/resolv.conf";

/// Filename of the byte-exact backup, inside the module's data directory.
const BACKUP_FILENAME: &str = "resolv.conf.backup";

/// Reads/writes a system `resolv.conf`-format file at an injectable path,
/// keeping a byte-exact backup alongside the crash marker.
///
/// Both the live file and the backup are written at mode 0600, matching
/// the Go implementation exactly (`os.WriteFile(..., 0o600)` for both).
/// That is unusually restrictive for `resolv.conf` — most systems ship it
/// world-readable (0644) so any unprivileged process can resolve names —
/// and is preserved here as a faithful port rather than silently loosened,
/// since it was not one of the two failure modes this port was asked to
/// fix. Flagged for the operator to reconsider, not fixed unilaterally.
pub struct ResolvConfBackend {
    resolv_conf_path: PathBuf,
    backup_path: PathBuf,
}

impl ResolvConfBackend {
    /// `resolv_conf_path` is the live file this backend reads/writes;
    /// `data_dir` is where its byte-exact backup (`resolv.conf.backup`)
    /// and the crash marker both live.
    pub fn new(resolv_conf_path: PathBuf, data_dir: &Path) -> ResolvConfBackend {
        ResolvConfBackend {
            resolv_conf_path,
            backup_path: data_dir.join(BACKUP_FILENAME),
        }
    }
}

#[async_trait]
impl PlatformBackend for ResolvConfBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let read = fs::read(&self.resolv_conf_path);
        let content = match read {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Nothing to back up or report — matches Go's `if content,
                // err := os.ReadFile(...); err == nil { write backup }`,
                // which silently skips the backup write on a missing file.
                return Ok(Vec::new());
            }
            Err(source) => {
                return Err(SysResolverError::Io {
                    context: format!("read {}", self.resolv_conf_path.display()),
                    source,
                });
            }
        };

        // Durable byte-exact backup, taken before the crash marker is
        // written by the caller — see `sysresolver::mod` for why this
        // ordering matters. A failure here aborts `apply()` before host
        // DNS is touched at all, rather than proceeding blind (a
        // deliberate strengthening over Go, which only logged a warning
        // and continued).
        write_owner_only(&self.backup_path, &content)?;

        Ok(parse_nameservers(&String::from_utf8_lossy(&content)))
    }

    async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let content = format_resolv_conf(servers);
        write_owner_only(&self.resolv_conf_path, content.as_bytes())
    }

    async fn restore(&self, fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let read = fs::read(&self.backup_path);
        let backup = match read {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                warn!(
                    path = %self.backup_path.display(),
                    "no byte-exact resolv.conf backup found; falling back to a bare server-list \
                     rewrite (search/options/comments from the original file are lost)"
                );
                return self.commit(fallback_servers).await;
            }
            Err(source) => {
                return Err(SysResolverError::Io {
                    context: format!("read {}", self.backup_path.display()),
                    source,
                });
            }
        };

        write_owner_only(&self.resolv_conf_path, &backup)?;

        let removed = fs::remove_file(&self.backup_path);
        if let Err(err) = removed {
            warn!(error = %err, path = %self.backup_path.display(), "failed to remove resolv.conf backup after restore");
        }
        Ok(())
    }

    async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let content =
            fs::read_to_string(&self.resolv_conf_path).map_err(|source| SysResolverError::Io {
                context: format!("read {}", self.resolv_conf_path.display()),
                source,
            })?;
        let servers = parse_nameservers(&content);
        if servers.is_empty() {
            return Err(SysResolverError::Backend(format!(
                "no nameservers found in {}",
                self.resolv_conf_path.display()
            )));
        }
        Ok(servers)
    }
}

/// Parses every `nameserver <addr>` line, skipping anything else
/// (comments, `search`, `options`, blank lines, unparseable addresses).
fn parse_nameservers(content: &str) -> Vec<IpAddr> {
    let mut servers = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("nameserver ") else {
            continue;
        };
        let parsed: Result<IpAddr, _> = rest.trim().parse();
        if let Ok(addr) = parsed {
            servers.push(addr);
        }
    }
    servers
}

/// Renders a minimal `resolv.conf`: one `nameserver` line per server, plus
/// a header comment. This is the format used both for `commit()` and as
/// the lossy fallback restore path — it necessarily drops any
/// `search`/`options`/comments the original file had.
fn format_resolv_conf(servers: &[IpAddr]) -> String {
    let mut lines = Vec::with_capacity(servers.len() + 1);
    lines.push("# Generated by penguin squawk module".to_string());
    for server in servers {
        lines.push(format!("nameserver {server}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    fn seed_resolv_conf(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("resolv.conf");
        fs::write(&path, content).expect("seed resolv.conf");
        path
    }

    #[tokio::test]
    async fn snapshot_backs_up_bytes_and_returns_parsed_servers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = "# comment\nnameserver 8.8.8.8\nsearch example.com\noptions ndots:2\nnameserver 8.8.4.4\n";
        let resolv_path = seed_resolv_conf(dir.path(), original);
        let backend = ResolvConfBackend::new(resolv_path, dir.path());

        let previous = backend.snapshot().await.expect("snapshot");
        assert_eq!(previous, vec![addr("8.8.8.8"), addr("8.8.4.4")]);

        let backup = fs::read_to_string(dir.path().join(BACKUP_FILENAME)).expect("read backup");
        assert_eq!(backup, original, "backup must be byte-exact");
    }

    #[tokio::test]
    async fn snapshot_of_missing_file_is_empty_and_writes_no_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = dir.path().join("resolv.conf"); // never created
        let backend = ResolvConfBackend::new(resolv_path, dir.path());

        let previous = backend.snapshot().await.expect("snapshot of missing file");
        assert!(previous.is_empty());
        assert!(!dir.path().join(BACKUP_FILENAME).exists());
    }

    #[tokio::test]
    async fn commit_writes_new_servers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "nameserver 8.8.8.8\n");
        let backend = ResolvConfBackend::new(resolv_path.clone(), dir.path());

        backend
            .commit(&[addr("1.1.1.1"), addr("1.0.0.1")])
            .await
            .expect("commit");

        let content = fs::read_to_string(&resolv_path).expect("read");
        assert!(content.contains("nameserver 1.1.1.1"));
        assert!(content.contains("nameserver 1.0.0.1"));
    }

    #[tokio::test]
    async fn restore_from_byte_backup_is_exact_including_search_and_options() {
        let dir = tempfile::tempdir().expect("tempdir");
        let original = "# a comment\nsearch example.com\nnameserver 8.8.8.8\noptions ndots:2\n";
        let resolv_path = seed_resolv_conf(dir.path(), original);
        let backend = ResolvConfBackend::new(resolv_path.clone(), dir.path());

        backend
            .snapshot()
            .await
            .expect("snapshot backs up original");
        backend
            .commit(&[addr("1.1.1.1")])
            .await
            .expect("commit new servers");
        backend.restore(&[addr("8.8.8.8")]).await.expect("restore");

        let restored = fs::read_to_string(&resolv_path).expect("read restored");
        assert_eq!(
            restored, original,
            "restore must reproduce the file byte-for-byte"
        );
        assert!(
            !dir.path().join(BACKUP_FILENAME).exists(),
            "backup is consumed on restore"
        );
    }

    #[tokio::test]
    async fn restore_without_byte_backup_falls_back_to_fallback_servers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "nameserver 1.1.1.1\n");
        let backend = ResolvConfBackend::new(resolv_path.clone(), dir.path());

        // No snapshot() call — no byte backup exists.
        backend
            .restore(&[addr("8.8.8.8"), addr("8.8.4.4")])
            .await
            .expect("restore falls back");

        let content = fs::read_to_string(&resolv_path).expect("read");
        assert!(content.contains("nameserver 8.8.8.8"));
        assert!(content.contains("nameserver 8.8.4.4"));
    }

    #[tokio::test]
    async fn current_reads_live_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "nameserver 9.9.9.9\n");
        let backend = ResolvConfBackend::new(resolv_path, dir.path());

        let current = backend.current().await.expect("current");
        assert_eq!(current, vec![addr("9.9.9.9")]);
    }

    #[tokio::test]
    async fn current_errors_when_no_nameservers_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "# nothing useful here\n");
        let backend = ResolvConfBackend::new(resolv_path, dir.path());

        let err = backend
            .current()
            .await
            .expect_err("no nameservers must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn live_file_and_backup_are_mode_0600() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "nameserver 8.8.8.8\n");
        let backend = ResolvConfBackend::new(resolv_path.clone(), dir.path());

        backend.snapshot().await.expect("snapshot");
        backend.commit(&[addr("1.1.1.1")]).await.expect("commit");

        let live_mode = fs::metadata(&resolv_path).expect("stat live").mode() & 0o777;
        let backup_mode = fs::metadata(dir.path().join(BACKUP_FILENAME))
            .expect("stat backup")
            .mode()
            & 0o777;
        assert_eq!(live_mode, 0o600);
        assert_eq!(backup_mode, 0o600);
    }

    #[tokio::test]
    async fn double_commit_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let resolv_path = seed_resolv_conf(dir.path(), "nameserver 8.8.8.8\n");
        let backend = ResolvConfBackend::new(resolv_path.clone(), dir.path());

        backend
            .commit(&[addr("1.1.1.1")])
            .await
            .expect("first commit");
        backend
            .commit(&[addr("1.1.1.1")])
            .await
            .expect("second commit");

        let content = fs::read_to_string(&resolv_path).expect("read");
        assert_eq!(content.matches("nameserver 1.1.1.1").count(), 1);
    }

    #[test]
    fn parse_nameservers_skips_comments_and_other_directives() {
        let content = "# comment\nsearch example.com\nnameserver 8.8.8.8\noptions ndots:2\nnameserver 8.8.4.4\n";
        assert_eq!(
            parse_nameservers(content),
            vec![addr("8.8.8.8"), addr("8.8.4.4")]
        );
    }
}
