//! BAL-driven parallel block execution.
//!
//! The engine uses this path when an Amsterdam block carries a decoded EIP-7928
//! Block-Level Access List (BAL). Workers execute transactions against revm's BAL state. The
//! main thread commits worker results to a canonical executor in transaction order.
//!
//! This path checks the BAL item-cost bound before state I/O. It validates the rebuilt
//! block-level BAL hash after post-execution. It does not yet run per-transaction fragment checks.
//! It does not yet report rich undeclared-access diagnostics.

pub mod error;
pub mod execute;
pub mod validation;

pub use error::RejectReason;
pub use execute::{BalExecutionError, BalExecutionOutput, BalPayloadExecutor, ReceiptFor};
pub use validation::check_item_count;
