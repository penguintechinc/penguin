//! Example external plugin written in Rust against `penguin-sdk`, mirroring
//! `go-client/examples/plugin-hello/main.go` closely enough to compare the
//! two directly.
//!
//! Beyond the `greet` command the Go original has, this plugin adds
//! `hostcheck`, which deliberately exercises the `HostServices` callback
//! leg the Go SDK has never been able to use (see `docs/PARITY.md` §1.10):
//! `init` logs a line and publishes an event through the host, and
//! `hostcheck` round-trips a value through the host's secret store and
//! reports what happened. The integration test in
//! `tests/hostservice_roundtrip.rs` asserts all three actually reached a
//! real `HostService` server.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;

use penguin_sdk::{
    CommandResult, CommandSpec, Event, EventType, HealthLevel, HealthReport, HostServices, Module,
    ModuleError, ModuleInfo, ModuleState, SecretError, Status,
};

/// The RPC-visible name of this module, matching Go's `plugin-hello` in
/// spirit (`"hello"`) but distinct so both binaries can be loaded side by
/// side during comparison testing.
const MODULE_NAME: &str = "hello-rs";
/// The module's own version, matching the Go example's `"1.0.0"`.
const MODULE_VERSION: &str = "1.0.0";
/// The secret key `hostcheck` round-trips through the host's secret store.
const HOSTCHECK_SECRET_KEY: &str = "hello-rs.hostcheck";

/// The example module. `host` is populated exactly once, in `init`, and read
/// from every later `dispatch("hostcheck", ...)` call — a `OnceLock` is the
/// interior-mutability cell `Module`'s doc comment names for exactly this
/// shape, since `init` takes `&self` rather than `&mut self`.
#[derive(Default)]
struct HelloRsModule {
    host: OnceLock<Arc<dyn HostServices>>,
}

#[async_trait]
impl Module for HelloRsModule {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: MODULE_NAME.to_string(),
            version: MODULE_VERSION.to_string(),
            description: "Example Rust external plugin that greets the user and proves the \
                          HostServices callback leg works"
                .to_string(),
            license_feature: String::new(),
        }
    }

    async fn init(&self, host: Arc<dyn HostServices>) -> Result<(), ModuleError> {
        host.logger()
            .info("hello-rs module initialised", &[("module", MODULE_NAME)]);
        host.events().publish(Event {
            module: MODULE_NAME.to_string(),
            event_type: EventType::Info,
            message: "hello-rs initialised".to_string(),
            at: SystemTime::now(),
            fields: HashMap::new(),
        });
        // OnceLock::set only fails if init somehow ran twice, which the
        // Module contract forbids; a second call would just mean our own
        // cached host handle stays the one from the first init.
        let _ = self.host.set(host);
        Ok(())
    }

    async fn start(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn stop(&self) -> Result<(), ModuleError> {
        Ok(())
    }

    async fn status(&self) -> Result<Status, ModuleError> {
        Ok(Status {
            state: ModuleState::Running,
            detail: HashMap::new(),
        })
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            level: HealthLevel::Healthy,
            message: "OK".to_string(),
            checked_at: SystemTime::now(),
        }
    }

    fn commands(&self) -> Vec<CommandSpec> {
        vec![
            CommandSpec {
                name: "greet".to_string(),
                use_line: "greet <name>".to_string(),
                short: "Greet someone".to_string(),
                flags: Vec::new(),
                subcommands: Vec::new(),
                tray: false,
                min_args: 1,
                max_args: 1,
            },
            CommandSpec {
                name: "hostcheck".to_string(),
                use_line: "hostcheck".to_string(),
                short: "Round-trip a secret through the host to prove HostServices works"
                    .to_string(),
                flags: Vec::new(),
                subcommands: Vec::new(),
                tray: false,
                min_args: 0,
                max_args: 0,
            },
        ]
    }

    async fn dispatch(
        &self,
        path: &[String],
        _flags: &HashMap<String, String>,
        args: &[String],
    ) -> Result<CommandResult, ModuleError> {
        let Some(command) = path.first() else {
            return Err(ModuleError::new("no command"));
        };
        match command.as_str() {
            "greet" => Ok(dispatch_greet(args)),
            "hostcheck" => Ok(self.dispatch_hostcheck().await),
            other => Err(ModuleError::new(format!("unknown command: {other}"))),
        }
    }

    fn config_schema(&self) -> Option<Vec<u8>> {
        // No configuration needed.
        None
    }
}

impl HelloRsModule {
    /// Round-trips [`HOSTCHECK_SECRET_KEY`] through `host.secrets()` and
    /// reports the outcome in plain text, so a test can assert on the exact
    /// string without needing its own gRPC client.
    async fn dispatch_hostcheck(&self) -> CommandResult {
        let Some(host) = self.host.get() else {
            return CommandResult {
                output: "hostcheck: no host services available".to_string(),
                json: Vec::new(),
                exit_code: 1,
            };
        };

        let secrets = host.secrets();
        let value = b"hostcheck-value".to_vec();
        let set_result = secrets.set(HOSTCHECK_SECRET_KEY, &value).await;
        if let Err(e) = set_result {
            return CommandResult {
                output: format!("hostcheck: secrets set failed: {e}"),
                json: Vec::new(),
                exit_code: 1,
            };
        }

        let get_result = secrets.get(HOSTCHECK_SECRET_KEY).await;
        report_get_result(get_result, &value)
    }
}

/// Builds the `hostcheck` command's [`CommandResult`] from the `get` half of
/// the round trip, split out from [`HelloRsModule::dispatch_hostcheck`] so
/// the success/mismatch/error cases each read as one line.
fn report_get_result(get_result: Result<Vec<u8>, SecretError>, expected: &[u8]) -> CommandResult {
    let Ok(got) = get_result else {
        let error = get_result.unwrap_err();
        return CommandResult {
            output: format!("hostcheck: secrets get failed: {error}"),
            json: Vec::new(),
            exit_code: 1,
        };
    };
    if got == expected {
        CommandResult {
            output: "hostcheck: secrets round-trip ok".to_string(),
            json: Vec::new(),
            exit_code: 0,
        }
    } else {
        CommandResult {
            output: format!("hostcheck: secrets round-trip mismatch: got {got:?}"),
            json: Vec::new(),
            exit_code: 1,
        }
    }
}

/// Implements the `greet <name>` command: exactly one positional argument is
/// required, and a wrong count is a usage failure signalled via exit code —
/// not a `ModuleError` — matching the Go original's contract exactly.
fn dispatch_greet(args: &[String]) -> CommandResult {
    let [name] = args else {
        return CommandResult {
            output: "usage: hello-rs greet <name>".to_string(),
            json: Vec::new(),
            exit_code: 1,
        };
    };
    CommandResult {
        output: format!("hello, {name}"),
        json: Vec::new(),
        exit_code: 0,
    }
}

/// Installs a minimal stderr-only tracing subscriber (never stdout — that is
/// reserved for the go-plugin handshake line) and hands the module to
/// [`penguin_sdk::plugin::serve`], which never returns.
fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    penguin_sdk::plugin::serve(Box::new(HelloRsModule::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_with_one_arg_succeeds() {
        let result = dispatch_greet(&["world".to_string()]);
        assert_eq!(result.output, "hello, world");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn greet_with_wrong_arg_count_is_a_usage_failure_not_an_error() {
        let result = dispatch_greet(&[]);
        assert_eq!(result.exit_code, 1);
        assert!(!result.output.is_empty());

        let result = dispatch_greet(&["a".to_string(), "b".to_string()]);
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn report_get_result_ok_matching_is_success() {
        let expected = b"value".to_vec();
        let result = report_get_result(Ok(expected.clone()), &expected);
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("round-trip ok"));
    }

    #[test]
    fn report_get_result_mismatch_is_a_failure() {
        let result = report_get_result(Ok(b"wrong".to_vec()), b"expected");
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("mismatch"));
    }

    #[test]
    fn report_get_result_error_is_a_failure() {
        let result = report_get_result(Err(SecretError::NotFound), b"expected");
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("secrets get failed"));
    }

    #[tokio::test]
    async fn info_reports_the_expected_identity() {
        let module = HelloRsModule::default();
        let info = module.info();
        assert_eq!(info.name, MODULE_NAME);
        assert_eq!(info.version, MODULE_VERSION);
        assert!(info.license_feature.is_empty());
    }

    #[tokio::test]
    async fn commands_declares_greet_and_hostcheck() {
        let module = HelloRsModule::default();
        let commands = module.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].name, "greet");
        assert_eq!(commands[0].min_args, 1);
        assert_eq!(commands[0].max_args, 1);
        assert_eq!(commands[1].name, "hostcheck");
    }

    #[tokio::test]
    async fn dispatch_unknown_command_is_a_module_error() {
        let module = HelloRsModule::default();
        let error = module
            .dispatch(&["bogus".to_string()], &HashMap::new(), &[])
            .await
            .unwrap_err();
        assert_eq!(error.message, "unknown command: bogus");
    }

    #[tokio::test]
    async fn dispatch_empty_path_is_a_module_error() {
        let module = HelloRsModule::default();
        let error = module
            .dispatch(&[], &HashMap::new(), &[])
            .await
            .unwrap_err();
        assert_eq!(error.message, "no command");
    }

    #[tokio::test]
    async fn hostcheck_before_init_reports_no_host_services() {
        let module = HelloRsModule::default();
        let result = module
            .dispatch(&["hostcheck".to_string()], &HashMap::new(), &[])
            .await
            .unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("no host services available"));
    }
}
