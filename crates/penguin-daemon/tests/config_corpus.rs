//! Runs the shared config corpus through the real config store and asserts each
//! verdict matches its fixture.
//!
//! The frozen Go client runs the identical corpus in
//! `go-client/internal/daemon/corpus_conformance_test.go`; the two engines must
//! agree on every case. That agreement is the M1 schema-parity gate.

use std::ffi::OsStr;
use std::path::PathBuf;

use penguin_daemon::config::ConfigStore;
use serde::Deserialize;
use tempfile::TempDir;

/// One corpus case, mirroring the JSON fixture shape.
#[derive(Deserialize)]
struct Case {
    description: String,
    valid: bool,
    schema: serde_json::Value,
    #[serde(rename = "instanceYaml")]
    instance_yaml: String,
}

/// Resolves the repo-root corpus directory from this crate's manifest dir.
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("testdata")
        .join("config-corpus")
}

#[test]
fn config_corpus_verdicts_match_the_fixtures() {
    let dir = corpus_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(iter) => iter,
        Err(err) => panic!("read corpus dir {}: {err}", dir.display()),
    };

    let mut checked = 0;
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }

        let raw = std::fs::read(&path).unwrap();
        let case: Case = match serde_json::from_slice(&raw) {
            Ok(parsed) => parsed,
            Err(err) => panic!("parse manifest {}: {err}", path.display()),
        };

        let temp = TempDir::new().unwrap();
        let modules = temp.path().join("modules.d");
        std::fs::create_dir_all(&modules).unwrap();
        std::fs::write(modules.join("mod.yaml"), &case.instance_yaml).unwrap();

        let schema_bytes = serde_json::to_vec(&case.schema).unwrap();
        let store = ConfigStore::new(temp.path());
        let result = store.module("mod", Some(&schema_bytes));

        assert_eq!(
            result.is_ok(),
            case.valid,
            "case {} ({}): expected valid={}, got {:?}",
            path.display(),
            case.description,
            case.valid,
            result,
        );
        checked += 1;
    }

    // Guard against a silently-empty corpus dir passing vacuously.
    assert!(
        checked >= 12,
        "expected the config corpus, only checked {checked} cases",
    );
}
