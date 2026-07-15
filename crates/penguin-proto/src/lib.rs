//! Generated gRPC/protobuf types for the penguin contracts.
//!
//! Everything here is emitted by `build.rs` from the canonical `proto/` tree —
//! there is no hand-written logic, which is why the crate is excluded from the
//! coverage gate. Hand-written conversions between these wire types and the
//! ergonomic `penguin-sdk` types live in `penguin-sdk`, not here.
//!
//! House-style lints are relaxed for this crate: generated code is not written
//! to our readability rules and must not fail `clippy -D warnings`.
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![allow(rustdoc::all)]

/// The daemon IPC contract (`penguin.daemon.v1`) served to the CLI and tray.
pub mod daemon {
    /// Version 1 of the daemon contract.
    pub mod v1 {
        tonic::include_proto!("penguin.daemon.v1");
    }
}

/// The module plugin contract (`penguin.sdk.v1`) shared with external plugins.
pub mod sdk {
    /// Version 1 of the module contract.
    pub mod v1 {
        tonic::include_proto!("penguin.sdk.v1");
    }
}
