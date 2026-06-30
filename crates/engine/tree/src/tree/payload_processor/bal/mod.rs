//! BAL-driven parallel block execution.
//!
//! The engine uses this path when an Amsterdam block carries a decoded EIP-7928
//! Block-Level Access List (BAL). Workers execute transactions against revm's BAL state. The
//! main thread commits worker results to a canonical executor in transaction order.
//!
//! Consensus validation checks the BAL item-cost bound before this path runs. This path streams
//! canonical state updates to a background BAL rebuild task for post-execution hash validation. It
//! does not yet run per-transaction fragment checks or report rich undeclared-access diagnostics.

mod ordered_outputs;
pub(crate) mod rebuild;
mod worker;

pub mod error;
pub mod execute;

pub use error::BalExecutionError;
pub use execute::execute_block;
