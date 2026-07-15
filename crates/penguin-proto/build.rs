//! Compiles the canonical `.proto` sources into Rust at build time.
//!
//! The single source of truth for every contract lives in the repo-root
//! `proto/` tree (the frozen Go client keeps byte-identical copies, guarded by
//! the `proto-drift` CI job). We resolve that tree relative to this crate so
//! the build works regardless of the caller's working directory.

use std::path::PathBuf;

/// Generates the daemon.v1 and sdk.v1 bindings. Additional contracts
/// (go-plugin broker/controller/stdio, squawk, health) are added in the
/// milestones that first consume them (M3/M5/M2) rather than up front, so no
/// generated-but-unused code accumulates.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // CARGO_MANIFEST_DIR is crates/penguin-proto; the proto root is two levels up.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir.join("..").join("..").join("proto");

    let daemon_proto = proto_root.join("penguin/daemon/v1/daemon.proto");
    let module_proto = proto_root.join("penguin/sdk/v1/module.proto");

    // Rebuild only when a source proto changes.
    println!("cargo:rerun-if-changed={}", daemon_proto.display());
    println!("cargo:rerun-if-changed={}", module_proto.display());

    let protos = [daemon_proto, module_proto];
    let include_dirs = [proto_root];

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&protos, &include_dirs)?;

    Ok(())
}
