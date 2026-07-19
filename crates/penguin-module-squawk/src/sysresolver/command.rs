//! A minimal external-process seam shared by the macOS (`networksetup`) and
//! Windows (`netsh`) backends — both just shell a fixed command with args
//! and inspect stdout/exit status. One trait, one fake, so neither backend
//! needs its own process-mocking machinery.
//!
//! Only compiled on macOS/Windows: nothing on Linux (this crate's only
//! buildable/testable target in this environment) references it, so it
//! stays out of that build entirely rather than sitting there as dead code.

use async_trait::async_trait;

use crate::sysresolver::error::SysResolverError;

/// The result of running an external command — enough for both backends to
/// decide success/failure and parse stdout, without exposing raw exit
/// codes or stderr that neither caller uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub success: bool,
    pub stdout: String,
}

/// Runs one external command to completion. The real implementation shells
/// out via `tokio::process::Command` ([`RealCommandRunner`]); every test
/// injects [`FakeCommandRunner`] so `networksetup`/`netsh` are never
/// actually executed.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, SysResolverError>;
}

/// Real process execution.
pub struct RealCommandRunner;

#[async_trait]
impl CommandRunner for RealCommandRunner {
    async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, SysResolverError> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await
            .map_err(|source| SysResolverError::Io {
                context: format!("run {program}"),
                source,
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(CommandOutput {
            success: output.status.success(),
            stdout,
        })
    }
}

/// Test double for [`CommandRunner`]: records every invocation and returns
/// a caller-configured, per-program-and-args response (or a default
/// success with empty stdout). Never spawns a real process.
#[cfg(test)]
pub struct FakeCommandRunner {
    pub calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    pub responses: std::sync::Mutex<std::collections::VecDeque<Result<CommandOutput, String>>>,
}

#[cfg(test)]
impl FakeCommandRunner {
    pub fn new() -> FakeCommandRunner {
        FakeCommandRunner {
            calls: std::sync::Mutex::new(Vec::new()),
            responses: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// Queues the next response, consumed in FIFO order by successive
    /// `run()` calls. Calls beyond the queued responses get a default
    /// success with empty stdout.
    pub fn push_response(&self, output: CommandOutput) {
        self.responses
            .lock()
            .expect("fake mutex poisoned")
            .push_back(Ok(output));
    }
}

#[cfg(test)]
impl Default for FakeCommandRunner {
    fn default() -> FakeCommandRunner {
        FakeCommandRunner::new()
    }
}

#[cfg(test)]
#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(&self, program: &str, args: &[String]) -> Result<CommandOutput, SysResolverError> {
        self.calls
            .lock()
            .expect("fake mutex poisoned")
            .push((program.to_string(), args.to_vec()));

        let queued = self
            .responses
            .lock()
            .expect("fake mutex poisoned")
            .pop_front();
        let Some(response) = queued else {
            return Ok(CommandOutput {
                success: true,
                stdout: String::new(),
            });
        };
        response.map_err(SysResolverError::Backend)
    }
}
