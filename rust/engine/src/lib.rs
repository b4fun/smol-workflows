//! Rust implementation of the smol-workflows engine.
//!
//! This crate contains the native Rust workflow engine, including the QuickJS
//! runtime, SQLite durable execution, and built-in agent providers.

pub mod agent_providers;
pub mod durable;
pub mod events;
pub mod js_runtime;
pub mod metadata;
pub mod workflow;

/// Current crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }
}
