//! Proof calculation version 2 using leaf data and cached branch hashes.

mod calculator;
pub use calculator::*;

mod metrics;
pub use metrics::ProofCursorMetrics;

mod multiproof_calculator;
pub use multiproof_calculator::*;

mod value;
pub use value::*;

mod node;

mod target;
pub(crate) use target::*;

/// Target to use with the `tracing` crate.
static TRACE_TARGET: &str = "trie::proof_v2";
