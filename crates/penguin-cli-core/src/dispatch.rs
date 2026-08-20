//! Turns a parsed module invocation into a `Dispatch` request, and folds the
//! resulting `DispatchChunk` stream back into stdout text and a process exit
//! code.
//!
//! Ported from `internal/cli.Builder.dispatch`
//! (`go-client/internal/cli/builder.go`). See `docs/PARITY.md` §1.11/§2.3
//! for the streaming divergences: Go silently treats a mid-stream transport
//! error the same as a clean end-of-stream (both just `break` the receive
//! loop), and the daemon today only ever sends one chunk anyway. Rust's
//! `tonic::Streaming` already distinguishes "clean end" (`Ok(None)`) from
//! "real failure" (`Err(status)`) at the type level, so `bins/penguin`'s
//! stream-driving loop surfaces the latter properly without needing to
//! replicate Go's conflation — nothing in this module has to reproduce it
//! either.

use clap::ArgMatches;

use crate::API_VERSION;
use crate::flags::collect_flags;
use crate::pb;
use crate::tree::ARGS_ID;

/// Walks a module's parsed `ArgMatches` down to the leaf subcommand that was
/// actually invoked, alongside the `CommandSpec` tree the matches were built
/// from, and produces the `Dispatch` request that leaf should send —
/// `module`, the subcommand chain within it (`path`), every explicitly-set
/// flag, and the positional arguments.
///
/// Returns `None` when `matches` has no subcommand invoked at all (bare
/// `penguin <module>` with nothing after it) or when a matched subcommand
/// name cannot be found in `specs`. The latter should not happen for a tree
/// this crate itself built via [`crate::tree::build_command_spec`], but is
/// data-shape validation, not a panic-worthy invariant.
pub fn resolve_dispatch(
    module: &str,
    specs: &[pb::CommandSpec],
    matches: &ArgMatches,
) -> Option<pb::DispatchRequest> {
    let (name, mut current_matches) = matches.subcommand()?;
    let mut current_spec = specs.iter().find(|spec| spec.name == name)?;
    let mut path = vec![current_spec.name.clone()];

    while let Some((sub_name, sub_matches)) = current_matches.subcommand() {
        let Some(next_spec) = current_spec
            .subcommands
            .iter()
            .find(|spec| spec.name == sub_name)
        else {
            break;
        };
        current_spec = next_spec;
        current_matches = sub_matches;
        path.push(current_spec.name.clone());
    }

    let flags = collect_flags(&current_spec.flags, current_matches);
    // `try_get_many` rather than `get_many`: a leaf whose `max_args` is `0`
    // has no `ARGS_ID` arg defined at all (see
    // `crate::tree::args_positional`), and `get_many` panics on an id the
    // command never declared — `try_get_many` reports that as `Err` instead,
    // which is exactly "this command takes no positional arguments".
    let args = current_matches
        .try_get_many::<String>(ARGS_ID)
        .ok()
        .flatten()
        .map(|values| values.cloned().collect())
        .unwrap_or_default();

    Some(pb::DispatchRequest {
        api_version: API_VERSION.to_string(),
        module: module.to_string(),
        path,
        flags,
        args,
    })
}

/// Substitutes piped stdin content for a command's positional argument when
/// none was parsed from the shell invocation — the mechanism that lets a
/// leaf command whose `CommandSpec` declares `max_args: 0` (so clap rejects
/// any literal shell token outright, see `crate::tree::args_positional`)
/// still receive a value: `echo "$KEY" | penguin waddleai key set` or a
/// hook shim piping its event JSON both resolve here.
///
/// A command that *does* declare a real positional (`max_args >= 1`) and
/// actually received one is untouched — `piped_stdin` is only ever
/// consulted when `args` came back empty. This exists specifically so a
/// secret or a sensitive payload never has to transit as a literal CLI
/// argument (readable in shell history and the process list for the
/// command's lifetime) — see `penguin_module_waddleai::commands`'s `key
/// set`/`hook` docs for the two call sites this was built for.
pub fn apply_stdin_fallback(args: Vec<String>, piped_stdin: Option<String>) -> Vec<String> {
    if !args.is_empty() {
        return args;
    }
    match piped_stdin {
        // A lone trailing newline is pipe/redirect noise (an `echo`, a text
        // editor's final newline) rather than payload content — trimmed the
        // same way shell command substitution (`$(...)`) strips it.
        Some(content) if !content.is_empty() => {
            vec![content.strip_suffix('\n').unwrap_or(&content).to_string()]
        }
        _ => Vec::new(),
    }
}

/// The text to write to stdout and the process exit code to report once a
/// `Dispatch` stream ends.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DispatchOutcome {
    /// Every chunk's `output`, concatenated in receive order.
    pub output: String,
    /// The `exit_code` carried by whichever chunk had `final` set — `0` if
    /// the stream ended without ever sending one, the same "never explicitly
    /// failed" default Go's `exitCode` local starts at.
    pub exit_code: i32,
}

/// Accumulates a `Dispatch` stream one chunk at a time, so a caller driving
/// the real (async) stream can print each chunk's text as it arrives —
/// matching Go's `fmt.Print(chunk.Output)` inside the receive loop — while
/// this type tracks the running exit code. [`fold_chunks`] applies the same
/// logic to an already-collected slice for tests that need no incremental
/// output.
#[derive(Debug, Clone, Default)]
pub struct ChunkAccumulator {
    exit_code: i32,
}

impl ChunkAccumulator {
    /// Starts a fresh accumulator at the "never explicitly failed" default
    /// exit code of `0`.
    pub fn new() -> ChunkAccumulator {
        ChunkAccumulator::default()
    }

    /// Records one received chunk, updating the running exit code if it is
    /// the final chunk, and returns the text it contributes to stdout.
    pub fn record<'a>(&mut self, chunk: &'a pb::DispatchChunk) -> &'a str {
        if chunk.r#final {
            self.exit_code = chunk.exit_code;
        }
        &chunk.output
    }

    /// The exit code to report once the stream has ended.
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

/// Folds an already-collected chunk sequence into a [`DispatchOutcome`] —
/// the hermetically-testable equivalent of driving [`ChunkAccumulator`] over
/// a real stream.
pub fn fold_chunks(chunks: &[pb::DispatchChunk]) -> DispatchOutcome {
    let mut accumulator = ChunkAccumulator::new();
    let mut output = String::new();
    for chunk in chunks {
        output.push_str(accumulator.record(chunk));
    }
    DispatchOutcome {
        output,
        exit_code: accumulator.exit_code(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::build_command_spec;
    use clap::Command;

    #[test]
    fn apply_stdin_fallback_uses_piped_content_when_no_args_were_parsed() {
        let args = apply_stdin_fallback(Vec::new(), Some("wa-secretvalue".to_string()));
        assert_eq!(args, vec!["wa-secretvalue".to_string()]);
    }

    #[test]
    fn apply_stdin_fallback_trims_exactly_one_trailing_newline() {
        let args = apply_stdin_fallback(Vec::new(), Some("payload\n".to_string()));
        assert_eq!(args, vec!["payload".to_string()]);
    }

    #[test]
    fn apply_stdin_fallback_preserves_internal_newlines_in_multiline_payloads() {
        let args = apply_stdin_fallback(Vec::new(), Some("{\n  \"a\": 1\n}\n".to_string()));
        assert_eq!(args, vec!["{\n  \"a\": 1\n}".to_string()]);
    }

    #[test]
    fn apply_stdin_fallback_never_overrides_explicitly_parsed_args() {
        let args = apply_stdin_fallback(
            vec!["already-typed".to_string()],
            Some("piped-content".to_string()),
        );
        assert_eq!(args, vec!["already-typed".to_string()]);
    }

    #[test]
    fn apply_stdin_fallback_with_no_piped_content_stays_empty() {
        assert_eq!(apply_stdin_fallback(Vec::new(), None), Vec::<String>::new());
    }

    #[test]
    fn apply_stdin_fallback_with_empty_piped_content_stays_empty() {
        assert_eq!(
            apply_stdin_fallback(Vec::new(), Some(String::new())),
            Vec::<String>::new()
        );
    }

    fn chunk(output: &str, r#final: bool, exit_code: i32) -> pb::DispatchChunk {
        pb::DispatchChunk {
            output: output.to_string(),
            json: Vec::new(),
            exit_code,
            r#final,
        }
    }

    fn nested_spec() -> pb::CommandSpec {
        pb::CommandSpec {
            name: "config".to_string(),
            max_args: 0,
            flags: vec![pb::FlagSpec {
                name: "verbose".to_string(),
                r#type: "bool".to_string(),
                ..Default::default()
            }],
            subcommands: vec![pb::CommandSpec {
                name: "show".to_string(),
                min_args: 0,
                max_args: -1,
                flags: vec![pb::FlagSpec {
                    name: "format".to_string(),
                    r#type: "string".to_string(),
                    default: "text".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn top_level_command_resolves_with_a_single_element_path() {
        let specs = vec![nested_spec()];
        let cmd = Command::new("test-module").subcommand(build_command_spec(&specs[0]));
        let matches = cmd
            .try_get_matches_from(["test-module", "config", "--verbose"])
            .expect("parse");

        let req = resolve_dispatch("test-module", &specs, &matches).expect("resolved");
        assert_eq!(req.module, "test-module");
        assert_eq!(req.path, vec!["config".to_string()]);
        assert_eq!(req.flags.get("verbose"), Some(&"true".to_string()));
        assert!(req.args.is_empty());
    }

    #[test]
    fn nested_command_resolves_with_the_full_path_chain() {
        let specs = vec![nested_spec()];
        let cmd = Command::new("test-module").subcommand(build_command_spec(&specs[0]));
        let matches = cmd
            .try_get_matches_from([
                "test-module",
                "config",
                "show",
                "--format",
                "json",
                "extra-arg",
            ])
            .expect("parse");

        let req = resolve_dispatch("test-module", &specs, &matches).expect("resolved");
        assert_eq!(req.path, vec!["config".to_string(), "show".to_string()]);
        assert_eq!(req.flags.get("format"), Some(&"json".to_string()));
        assert_eq!(req.args, vec!["extra-arg".to_string()]);
    }

    #[test]
    fn no_subcommand_invoked_resolves_to_none() {
        let specs = vec![nested_spec()];
        let cmd = Command::new("test-module").subcommand(build_command_spec(&specs[0]));
        let matches = cmd.try_get_matches_from(["test-module"]).expect("parse");

        assert!(resolve_dispatch("test-module", &specs, &matches).is_none());
    }

    #[test]
    fn fold_chunks_concatenates_output_in_order() {
        let chunks = vec![chunk("hello ", false, 0), chunk("world\n", true, 0)];
        let outcome = fold_chunks(&chunks);
        assert_eq!(outcome.output, "hello world\n");
        assert_eq!(outcome.exit_code, 0);
    }

    #[test]
    fn exit_code_comes_from_the_final_chunk_only() {
        let chunks = vec![chunk("ignored\n", false, 99), chunk("real\n", true, 3)];
        let outcome = fold_chunks(&chunks);
        assert_eq!(outcome.exit_code, 3);
    }

    #[test]
    fn nonzero_exit_code_propagates() {
        let chunks = vec![chunk("failed\n", true, 7)];
        assert_eq!(fold_chunks(&chunks).exit_code, 7);
    }

    #[test]
    fn a_stream_with_no_final_chunk_defaults_to_exit_code_zero() {
        let chunks = vec![chunk("partial\n", false, 0)];
        assert_eq!(fold_chunks(&chunks).exit_code, 0);
    }

    #[test]
    fn empty_stream_yields_empty_output_and_exit_zero() {
        let outcome = fold_chunks(&[]);
        assert_eq!(outcome, DispatchOutcome::default());
    }

    #[test]
    fn accumulator_matches_fold_chunks_when_driven_incrementally() {
        let chunks = vec![
            chunk("a", false, 0),
            chunk("b", false, 0),
            chunk("c", true, 5),
        ];
        let mut accumulator = ChunkAccumulator::new();
        let mut output = String::new();
        for c in &chunks {
            output.push_str(accumulator.record(c));
        }
        assert_eq!(output, "abc");
        assert_eq!(accumulator.exit_code(), 5);
        assert_eq!(
            fold_chunks(&chunks),
            DispatchOutcome {
                output,
                exit_code: 5
            }
        );
    }
}
