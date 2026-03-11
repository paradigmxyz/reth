//! Backward-compatible re-exports for BAL store types.
//!
//! The concrete BAL store implementation lives in `reth-bal-store` so both the
//! Engine API and networking stack can share the same abstraction without
//! introducing a `network -> rpc` dependency.

pub use reth_bal_store::*;
