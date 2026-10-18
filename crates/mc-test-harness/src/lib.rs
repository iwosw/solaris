//! # mc-test-harness
//!
//! Diff testing infrastructure.
//!
//! Part of the Solaris engine. The raw-TCP wire client and the vanilla-jar
//! content capture are production code owned by `mc_vanilla`; this crate
//! consumes them.

pub mod parity;
pub mod replay;

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
