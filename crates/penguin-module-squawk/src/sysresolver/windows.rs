//! Windows backend: `netsh interface ip set dnsservers` per interface.
//!
//! **Not exercised by any build in this port's environment** — same caveat
//! as `macos.rs`: this crate is only ever built/tested on Linux here, so
//! this file has never actually been compiled by the agent that wrote it.
//! Ported faithfully from the Go source but flagged rather than
//! claimed-verified.
//!
//! Restore resets each interface to `dhcp`, discarding the prior static
//! configuration — same lossy-restore caveat as the macOS backend; see its
//! module doc for why capturing "was this interface on DHCP or static DNS
//! before" is out of scope here (Go's implementation has the same
//! limitation).

use std::net::IpAddr;

use async_trait::async_trait;
use tracing::warn;

use crate::sysresolver::backend::PlatformBackend;
use crate::sysresolver::command::{CommandRunner, RealCommandRunner};
use crate::sysresolver::error::SysResolverError;

/// Identifier persisted in the crash marker for this backend.
pub const BACKEND_NAME: &str = "netsh";

pub struct NetshBackend {
    runner: Box<dyn CommandRunner>,
}

impl NetshBackend {
    pub fn new(runner: Box<dyn CommandRunner>) -> NetshBackend {
        NetshBackend { runner }
    }

    async fn interfaces(&self) -> Result<Vec<String>, SysResolverError> {
        let args = vec![
            "interface".to_string(),
            "show".to_string(),
            "interface".to_string(),
        ];
        let output = self.runner.run("netsh", &args).await?;

        let mut interfaces = Vec::new();
        for line in output.stdout.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("Admin State") {
                continue;
            }
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 4 {
                continue;
            }
            let Some(name) = fields.last() else {
                continue;
            };
            interfaces.push((*name).to_string());
        }
        Ok(interfaces)
    }
}

/// Builds the Windows backend list for [`super::SysResolver::new`]: a
/// single `netsh`-backed entry.
pub fn build_backends() -> Vec<Box<dyn PlatformBackend>> {
    vec![Box::new(NetshBackend::new(Box::new(RealCommandRunner)))]
}

#[async_trait]
impl PlatformBackend for NetshBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn snapshot(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let current = self.current().await;
        Ok(current.unwrap_or_default())
    }

    async fn commit(&self, servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let interfaces = self.interfaces().await?;
        if interfaces.is_empty() {
            return Err(SysResolverError::Backend(
                "no network interfaces found".to_string(),
            ));
        }
        let Some(primary) = servers.first() else {
            return Err(SysResolverError::NoServers);
        };

        for iface in &interfaces {
            let mut args = vec![
                "interface".to_string(),
                "ip".to_string(),
                "set".to_string(),
                "dnsservers".to_string(),
                format!("name={iface}"),
                "static".to_string(),
                primary.to_string(),
            ];
            if let Some(secondary) = servers.get(1) {
                args.push("index=2".to_string());
                args.push(secondary.to_string());
            }
            let result = self.runner.run("netsh", &args).await;
            if let Err(err) = result {
                warn!(interface = %iface, error = %err, "failed to set DNS for interface");
            }
        }
        Ok(())
    }

    async fn restore(&self, _fallback_servers: &[IpAddr]) -> Result<(), SysResolverError> {
        let interfaces = self.interfaces().await?;
        for iface in &interfaces {
            let args = vec![
                "interface".to_string(),
                "ip".to_string(),
                "set".to_string(),
                "dnsservers".to_string(),
                format!("name={iface}"),
                "dhcp".to_string(),
            ];
            let result = self.runner.run("netsh", &args).await;
            if let Err(err) = result {
                warn!(interface = %iface, error = %err, "failed to restore DNS for interface");
            }
        }
        Ok(())
    }

    async fn current(&self) -> Result<Vec<IpAddr>, SysResolverError> {
        let interfaces = self.interfaces().await?;
        let Some(first) = interfaces.first() else {
            return Err(SysResolverError::Backend(
                "no network interfaces found".to_string(),
            ));
        };

        let args = vec![
            "interface".to_string(),
            "ip".to_string(),
            "show".to_string(),
            "dns".to_string(),
            first.clone(),
        ];
        let output = self.runner.run("netsh", &args).await?;

        let mut servers = Vec::new();
        for line in output.stdout.lines() {
            let line = line.trim();
            let Some((_, rest)) = line.split_once(':') else {
                continue;
            };
            let parsed: Result<IpAddr, _> = rest.trim().parse();
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
    ) -> (NetshBackend, std::sync::Arc<FakeCommandRunner>) {
        let runner = std::sync::Arc::new(runner);
        let boxed: Box<dyn CommandRunner> = Box::new(SharedRunner(runner.clone()));
        (NetshBackend::new(boxed), runner)
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

    const INTERFACE_LISTING: &str = "Admin State    State          Type             Interface Name\n\
        --------------------------------------------------------------\n\
        Enabled        Connected      Dedicated        Ethernet\n";

    #[tokio::test]
    async fn commit_sets_static_dns_with_primary_and_secondary() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: INTERFACE_LISTING.to_string(),
        });
        let (backend, fake) = backend_with(fake);

        backend
            .commit(&[addr("1.1.1.1"), addr("1.0.0.1")])
            .await
            .expect("commit");

        let calls = fake.calls.lock().expect("lock");
        assert_eq!(calls.len(), 2);
        assert!(calls[1].1.contains(&"static".to_string()));
        assert!(calls[1].1.contains(&"1.1.1.1".to_string()));
        assert!(calls[1].1.contains(&"index=2".to_string()));
        assert!(calls[1].1.contains(&"1.0.0.1".to_string()));
    }

    #[tokio::test]
    async fn restore_sets_dhcp_for_every_interface() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: INTERFACE_LISTING.to_string(),
        });
        let (backend, fake) = backend_with(fake);

        backend.restore(&[addr("8.8.8.8")]).await.expect("restore");

        let calls = fake.calls.lock().expect("lock");
        assert!(calls[1].1.contains(&"dhcp".to_string()));
    }

    #[tokio::test]
    async fn current_parses_colon_separated_dns_output() {
        // Mirrors the Go parser exactly, including its limitation: only
        // lines that themselves contain a `:` are considered, so a
        // continuation line (real `netsh` wraps a second server onto its
        // own line with no colon) would not be picked up either. Every
        // line here carries its own colon so both servers parse.
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: INTERFACE_LISTING.to_string(),
        });
        fake.push_response(CommandOutput {
            success: true,
            stdout: "Statically Configured DNS Servers:    8.8.8.8\nDNS Servers:    8.8.4.4\n"
                .to_string(),
        });
        let (backend, _fake) = backend_with(fake);

        let current = backend.current().await.expect("current");
        assert_eq!(current, vec![addr("8.8.8.8"), addr("8.8.4.4")]);
    }

    #[tokio::test]
    async fn commit_with_no_interfaces_errors() {
        let fake = FakeCommandRunner::new();
        fake.push_response(CommandOutput {
            success: true,
            stdout: String::new(),
        });
        let (backend, _fake) = backend_with(fake);

        let err = backend
            .commit(&[addr("1.1.1.1")])
            .await
            .expect_err("no interfaces must error");
        assert!(matches!(err, SysResolverError::Backend(_)));
    }
}
