//! Types and a local binary plugin client for smol-workflows sandboxes.
//!
//! Protocol v1 is JSON-first and local-only: the workflow runner invokes a
//! provider executable on the same machine, writes one JSON request to stdin,
//! reads one JSON response from stdout, and treats stderr as diagnostics.

pub mod plugin;
pub mod v1;

pub use plugin::{PluginClientError, SandboxProviderPlugin};
pub use v1::*;
