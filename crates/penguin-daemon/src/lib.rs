//! penguind core.
//!
//! M1 lands only the configuration store — daemon and module config loading
//! plus JSON-Schema validation, the piece the config-conformance gate exercises
//! against the frozen Go client. The supervisor state machine, event broker,
//! and Daemon gRPC service arrive in M2.

pub mod config;
