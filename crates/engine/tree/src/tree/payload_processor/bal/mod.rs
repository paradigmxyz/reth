//! BAL-driven parallel block execution.
//!
//! The engine uses this path when an Amsterdam block carries a decoded EIP-7928
//! Block-Level Access List (BAL). Workers execute transactions against BAL state. The
//! main thread commits worker results to a canonical executor in transaction order.
//!
//! Consensus validation checks the BAL item-cost bound before this path runs. This path validates
//! the rebuilt block-level BAL hash after post-execution. It does not yet run per-transaction
//! fragment checks. It does not yet report rich undeclared-access diagnostics.

mod ordered_outputs;
mod worker;

pub mod error;
pub mod execute;

pub use error::BalExecutionError;
pub use execute::execute_block;
