//! macOS backend: `networksetup -setdnsservers` per network service.
//!
//! **Not exercised by any build in this port's environment** — the crate
//! is only ever built/tested on Linux here (see the repo's Docker-only
//! build constraint), so this file, unlike `linux/`, has never actually
//! been compiled by the agent that wrote it. Written carefully and ported
//! faithfully from the Go source, but flagged rather than claimed-verified.
//!
//! Restore resets each service to `"Empty"` (DHCP-assigned DNS), not the
//! exact prior static configuration — this discards whatever
//! `previous_servers` the crash marker recorded. That is lossy and matches
//! the Go implementation's actual behaviour; it is flagged here rather than
//! silently reproduced, per the porting brief. A byte-exact restore would
//! need to capture each service's *DNS configuration method* (DHCP vs.
//! static-with-which-servers) before applying, which `networksetup
//! -getdnsservers` cannot distinguish — it reports the same shape of
//! output whether a service is on DHCP-provided or explicit DNS. Solving
//! that faithfully needs `scutil --dns` or the SystemConfiguration
//! framework, out of scope for this port.

use std::net::IpAddr;

use async_trait::async_trait;
use tracing::warn;

use crate::sysresolver::backend::PlatformBackend;
use crate::sysresolver::command::{CommandRunner, RealCommandRunner};
use crate::sysresolver::error::SysResolverError;

/// Identifier persisted in the crash marker for this backend.
pub const BACKEND_NAME: &str = "networksetup";

/// Network service names containing this substring (case-insensitive) are
/// skipped, matching the Go implementation — PPP services don't support
/// `-setdnsservers` the same way.
const SKIP_SUBSTRING: &str = "ppp";

pub struct NetworkSetupBackend {
    runner: Box<dyn CommandRunner>,
}

impl NetworkSetupBackend {
    pub fn new(runner: Box<dyn CommandRunner>) -> NetworkSetupBackend {
        NetworkSetupBackend { runner }
    }

    async fn network_services(&self) -> Result<Vec<String>, SysResolverError> {
        let args = vec!["-listallnetworkservices".to_string()];
        let output = self.runner.run("networksetup", &args).await?;
        let mut services = Vec::new();
        for line in output.stdout.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('(') {
                continue;
            }
            services.push(line.to_string());
        }
        Ok(services)
    }
}

/// Builds the macOS backend list for [`super::SysResolver::new`]: a single
/// `networksetup`-backed entry — macOS has no equivalent of
/// systemd-resolved's D-Bus API to prefer over it.
pub fn build_backends() -> Vec<Box<dyn PlatformBackend>> {
    vec![Box::new(NetworkSetupBackend::new(Box::new(
        RealCommandRunner,
    )))]
}

#[async_trait]
impl PlatformBackend for NetworkSetupBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let current = self.current().await;
        Ok(current.unwrap_or_default())
    }

    async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let services = self.network_services().await?;
        if services.is_empty() {
            return Err(SysResolverError::Backend(
                "no network services found".to_string(),
            ));
        }

        let mut server_strings = Vec::with_capacity(servers.len());
        for server in servers {
            server_strings.push(server.to_string());
        }

        for service in &services {
            if service.to_lowercase().contains(SKIP_SUBSTRING) {
                continue;
            }
            let mut args = vec!["-setdnsservers".to_string(), service.clone()];
            args.extend(server_strings.iter().cloned());
            let result = self.runner.run("networksetup", &args).await;
            if let Err(err) = result {
                warn!(service = %service, error = %err, "failed to set DNS for network service");
            }
        }
        Ok(())
    }

    async fn restore(&self, _fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let services = self.network_services().await?;
        for service in &services {
            if service.to_lowercase().contains(SKIP_SUBSTRING) {
                continue;
            }
            let args = vec![
                "-setdnsservers".to_string(),
                service.clone(),
                "Empty".to_string(),
            ];
            let result = self.runner.run("networksetup", &args).await;
            if let Err(err) = result {
                warn!(service = %service, error = %err, "failed to restore DNS for network service");
            }
        }
        Ok(())
    }

    async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let services = self.network_services().await?;
        let Some(first) = services.first() else {
            return Err(SysResolverError::Backend(
                "no network services found".to_string(),
            ));
        };

        let args = vec!["-getdnsservers".to_string(), first.clone()];
        let output = self.runner.run("networksetup", &args).await?;

        let mut servers = Vec::new();
        for line in output.stdout.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("DNS") || line.starts_with("There aren't any") {
                continue;
            }
            let parsed: Result<IpAddr, _> = line.parse();
            if let Ok(addr) = parsed {
                servers.push(addr);
            }
        }

        if servers.is_empty() {
            return Err(SysResolverError::Backend(
                "no DNS servers configured".to_string(),
            ));
        }
        Ok(servers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sysresolver::command::{CommandOutput, FakeCommandRunner};

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("valid test address")
    }

    fn backend_with(
        runner: FakeCommandRunner,
    ) -> (NetworkSetupBackend, std::sync::Arc<FakeCommandRunner>) {
        let runner = std::sync::Arc::new(runner);
        let boxed: Box<dyn CommandRunner> = Box::new(SharedRunner(runner.clone()));
        (NetworkSetupBackend::new(boxed), runner)
    }

    struct SharedRunner(std::sync::Arc<FakeCommandRunner>);

    #[async_trait]
    impl CommandRunner for SharedRunner {
        async fn run(
            &self,
            program: &str,
            args: &[String],
        ) -> Result<CommandOutput, SysResolverError> {
            self.0.run(program, args).await
        }
    }

    #[tokio::test]
    async fn commit_skips_ppp_services_and_passes_every_server() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout:
                "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\nPPP (WAN)\n"
                    .to_string(),
        });
        let (backend, fake) = backend_with(fake);

        backend
            .commit(&[addr("1.1.1.1"), addr("1.0.0.1")])
            .await
            .expect("commit");

        let calls = fake.calls.lock().expect("lock");
        // First call lists services; only the non-PPP one gets a set call.
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].0, "networksetup");
        assert_eq!(calls[1].1[0], "-setdnsservers");
        assert_eq!(calls[1].1[1], "Wi-Fi");
        assert!(calls[1].1.contains(&"1.1.1.1".to_string()));
        assert!(calls[1].1.contains(&"1.0.0.1".to_string()));
    }

    #[tokio::test]
    async fn restore_sets_empty_for_every_non_ppp_service() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: "Wi-Fi\nEthernet\n".to_string(),
        });
        let (backend, fake) = backend_with(fake);

        backend.restore(&[addr("8.8.8.8")]).await.expect("restore");

        let calls = fake.calls.lock().expect("lock");
        assert_eq!(calls.len(), 3); // list + 2 services
        assert!(calls[1].1.contains(&"Empty".to_string()));
        assert!(calls[2].1.contains(&"Empty".to_string()));
    }

    #[tokio::test]
    async fn current_parses_dns_server_list_output() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: "Wi-Fi\n".to_string(),
        });
        fake.push_response(CommandOutput {
            success: true,
            stdout: "8.8.8.8\n8.8.4.4\n".to_string(),
        });
        let (backend, _fake) = backend_with(fake);

        let current = backend.current().await.expect("current");
        assert_eq!(current, vec![addr("8.8.8.8"), addr("8.8.4.4")]);
    }

    #[tokio::test]
    async fn current_with_no_services_errors() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: String::new(),
        });
        let (backend, _fake) = backend_with(fake);

        let err = backend.current().await.expect_err("no services must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }
}
