//! Compiles the canonical `.proto` sources into Rust at build time.
//!
//! The single source of truth for every contract lives in the repo-root
//! `proto/` tree (the frozen Go client keeps byte-identical copies, guarded by
//! the `proto-drift` CI job). We resolve that tree relative to this crate so
//! the build works regardless of the caller's working directory.

use std::path::PathBuf;

/// Generates the daemon.v1, sdk.v1, and vendored go-plugin bindings.
///
/// The squawk contract is added in M5, when the module that speaks it lands;
/// `grpc.health.v1` is never generated here because `tonic-health` already
/// ships it. Contracts are wired up in the milestone that first consumes them
/// so no generated-but-unused code accumulates.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // CARGO_MANIFEST_DIR is crates/penguin-proto; the proto root is two levels up.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest_dir.join("..").join("..").join("proto");

    let daemon_proto = proto_root.join("penguin/daemon/v1/daemon.proto");
    let module_proto = proto_root.join("penguin/sdk/v1/module.proto");
    // Desktop client session proxy service — separate package to avoid proto-drift
    // conflicts with the frozen daemon.proto (kept byte-identical for Go client).
    let desktop_proxy_proto = proto_root.join("penguin/desktop/v1/proxy.proto");
    // Vendored hashicorp/go-plugin v1.7.0 — the protocol the Rust host must
    // speak so existing Go-built plugin binaries load unchanged.
    let broker_proto = proto_root.join("goplugin/grpc_broker.proto");
    let controller_proto = proto_root.join("goplugin/grpc_controller.proto");
    let stdio_proto = proto_root.join("goplugin/grpc_stdio.proto");

    let protos = [
        daemon_proto,
        module_proto,
        desktop_proxy_proto,
        broker_proto,
        controller_proto,
        stdio_proto,
    ];

    // Rebuild only when a source proto changes.
    for proto in &protos {
        println!("cargo:rerun-if-changed={}", proto.display());
    }

    // Add both the local proto root and the standard google protobuf includes
    let mut include_dirs = vec![proto_root];

    // Add google protobuf standard includes (from grpc-tools or system install)
    if let Ok(home) = std::env::var("HOME") {
        let grpc_tools_path =
            PathBuf::from(home).join(".local/lib/python3.12/site-packages/grpc_tools/_proto");
        if grpc_tools_path.exists() {
            include_dirs.push(grpc_tools_path);
        }
    }

    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&protos, &include_dirs)?;

    Ok(())
}
