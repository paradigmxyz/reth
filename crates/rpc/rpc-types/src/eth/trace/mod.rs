//! Types for tracing

pub mod filter;
pub mod parity;

/// Geth tracing types
pub mod geth {
    // re-exported for geth tracing types
    pub use ethers_core::types::{DefaultFrame, GethDebugTracingOptions, StructLog};
}
