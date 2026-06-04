//! BAL-driven parallel block execution placeholders.
//!
//! Block-Level Access List execution is Amsterdam-only and out of scope for the active evm2
//! pre-Amsterdam sync path. Keep the module boundary in place so Amsterdam support can be rebuilt
//! here without reintroducing the old executor backend.

mod ordered_outputs;
mod worker;

pub mod error;
pub mod execute;

pub use error::BalExecutionError;
pub use execute::execute_block;
