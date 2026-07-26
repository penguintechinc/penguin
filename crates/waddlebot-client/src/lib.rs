//! An async REST client for the waddlebot hub API's `/api/v1` surface —
//! the protocol layer `penguin-module-waddlebot` wraps. This crate owns
//! request/response typing and error handling only; the module + local
//! integration bridge (built on a separate track) own CLI plumbing, config
//! storage, and anything module-lifecycle-shaped.
//!
//! Authentication is a Community Access Token (CAT, `wdl_c_<hex>`) sent as
//! `Authorization: Bearer <cat>` — the credential kind `waddlebot#155`
//! identifies as the right fit for an unattended, community-scoped client
//! like this one, as opposed to a full user session. See
//! [`error::WaddlebotError::Auth`]'s doc comment for that issue's actual
//! finding: the CAT *resolver* exists server-side but `requireAuth` never
//! calls it, so every CAT-authenticated request 401s today regardless of
//! token validity. This crate implements the endpoints to their intended
//! contract anyway — there's no client-side fix for a server-side auth
//! bug, and the module built on top of this crate needs the real surface
//! ready for when `requireAuth` is wired up.
//!
//! `workflows` and `loyalty` are opaque JSON proxies on the hub side
//! (forwarded verbatim to their own backing microservices) — see
//! [`client::WaddlebotClient`]'s workflow/loyalty methods, which take and
//! return [`serde_json::Value`] rather than typed structs, since there's no
//! hub-owned schema to model.

pub mod client;
pub mod config;
pub mod error;
pub mod models;
mod tls;

pub use client::WaddlebotClient;
pub use config::Config;
pub use error::{ErrorBody, WaddlebotError};
