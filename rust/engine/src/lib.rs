//! Rust implementation of the smol-workflows engine.
//!
//! This crate is intentionally minimal for now. The TypeScript engine in
//! `ts/engine` will be ported here incrementally.

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
