//! BAL-driven parallel block execution.
//!
//! An alternative to the serial `execute_block` path, activated when a block carries a
//! Block-Level Access List (EIP-7928). The BAL declares every storage slot, account field, and
//! code access a block makes, which lets us:
//!
//! - Materialize an exhaustive in-memory pre-state snapshot before execution begins.
//! - Execute transactions in parallel on isolated EVM instances, each using the BAL to resolve
//!   mid-block state lookups via revm's `BalDatabase`.
//! - Commit worker outputs to a canonical `BlockExecutor` in tx order, preserving client-override
//!   extension points (`apply_pre_execution_changes`, `commit_transaction`,
//!   `apply_post_execution_changes`).
//!
//! See `BAL.md` at the repo root for the full design.
//!
//! This module is being built leaf-first. Currently only the data types and error variants are in
//! place; orchestration follows in subsequent PRs.

pub mod error;
pub mod execute;
pub mod feasibility;
pub mod pre_state;
pub mod snapshot;
pub mod snapshot_db;
pub mod sparse_trie_stream;
pub mod validation;
pub mod worker;

pub use error::RejectReason;
pub use execute::{BalExecutionError, BalExecutionOutput, BalPayloadExecutor, ReceiptFor};
pub use pre_state::{BlockPreState, RequiredReads};
pub use snapshot::build_pre_state;
pub use snapshot_db::{SnapshotDatabase, SnapshotDbError};
pub use sparse_trie_stream::spawn_stream_bal_to_sparse_trie;
pub use validation::{check_bal_hash, check_item_count, BAL_ITEM_COST};
pub use worker::{BalBlockExecutor, WorkerError};
