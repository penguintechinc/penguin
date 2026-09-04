//! Pure, unit-testable tray menu model — no display, no event loop, no I/O.
//!
//! System tray code is normally the least-tested part of an application: it
//! needs a display server, a native event loop, and platform toolkit APIs a
//! CI runner does not have. This crate exists to pull every decision that
//! *doesn't* need those things out of the platform shell and into plain data
//! transformations, so the M7 platform shells (Linux ksni, macOS/Windows
//! native tray) can stay dumb renderers: call [`build_menu`], walk the
//! result, forward whichever [`Action`] the user clicked.
//!
//! - [`DaemonConnection`] models whether the daemon is reachable and, if so,
//!   what it reported for each module ([`ModuleInput`]).
//! - [`build_menu`] turns that into a [`Menu`] tree of [`MenuItem`] rows.
//! - [`Action`] is what clicking a row means; the shell only ever matches on
//!   it and issues the corresponding RPC or process exit.
//!
//! The only dependency is `penguin-sdk`, for the [`penguin_sdk::ModuleState`],
//! [`penguin_sdk::HealthLevel`], and [`penguin_sdk::CommandSpec`] types the
//! daemon already exposes — no GUI toolkit, no gRPC transport, nothing that
//! requires a display to build or test.

pub mod action;
pub mod menu;
pub mod module;
pub mod severity;

pub use action::Action;
pub use menu::{DaemonConnection, Menu, MenuItem, build_menu};
pub use module::ModuleInput;
pub use severity::Severity;
