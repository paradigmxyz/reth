//! BAL-driven parallel block execution.
//!
//! An alternative to the serial `execute_block` path, activated when a block carries a
//! Block-Level Access List (EIP-7928). The BAL declares every storage slot, account field, and
//! code access a block makes, which lets us:
//!
//! - Execute transactions in parallel on isolated EVM instances, each using the BAL to resolve
//!   mid-block state lookups via revm's BAL state.
//! - Commit worker outputs to a canonical `BlockExecutor` in tx order, preserving client-override
//!   extension points (`apply_pre_execution_changes`, `commit_transaction`,
//!   `apply_post_execution_changes`).
//! - Feed sparse-trie state-root work from the BAL through the payload prewarm task.
//!
//! See `BAL.md` at the repo root for the full design.
//!
//! The core BAL executor and its validation/state-root helpers live here. Engine-thread
//! integration remains gated elsewhere.

pub mod error;
pub mod execute;
#[cfg(test)]
pub mod pre_state;
#[cfg(test)]
pub mod snapshot;
#[cfg(test)]
pub mod snapshot_db;
pub mod validation;
#[cfg(test)]
pub mod worker;

pub use error::RejectReason;
pub use execute::{BalExecutionError, BalExecutionOutput, BalPayloadExecutor, ReceiptFor};
#[cfg(test)]
pub use pre_state::{BlockPreState, RequiredReads};
#[cfg(test)]
pub use snapshot::build_pre_state;
#[cfg(test)]
pub use snapshot_db::{SnapshotDatabase, SnapshotDbError};
pub use validation::{check_bal_hash, check_item_count};
#[cfg(test)]
pub use worker::{BalBlockExecutor, WorkerError};
