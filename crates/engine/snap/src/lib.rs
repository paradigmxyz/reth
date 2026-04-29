//! Engine-driven snap sync orchestrator for snap/2 (EIP-8189).
//!
//! This crate implements a standalone snap sync process driven by the engine tree,
//! not the staged pipeline. Snap sync is a live, reactive process that responds to
//! chain advancement in real-time via events forwarded from the engine.

pub mod bal;
pub mod controller;
pub mod download;
pub mod finalize;
pub mod orchestrator;
pub mod pivot;
pub mod serve;
pub mod storage;

mod proof;

use alloy_primitives::{Bytes, B256};

/// How many blocks behind HEAD to place the snap sync pivot.
///
/// The serving node reverse-applies changesets to reconstruct hashed state at
/// HEAD−N, so this must be large enough that the target block's hashed state
/// is always fully persisted to MDBX (the engine keeps ~2 blocks in memory).
pub const PIVOT_OFFSET: u64 = 16;

/// Soft response size limit for snap protocol requests (2 MiB).
pub const SNAP_RESPONSE_BYTES_LIMIT: u64 = 2 * 1024 * 1024;

/// Events sent from the engine tree to the snap sync orchestrator.
#[derive(Debug, Clone)]
pub enum SnapSyncEvent {
    /// A new block was received via `new_payload`. The BAL bytes come from `ExecutionPayloadV4`.
    NewBlock {
        /// Block number.
        number: u64,
        /// Block hash.
        hash: B256,
        /// State root from the block header.
        state_root: B256,
        /// Parent block hash.
        parent_hash: B256,
        /// RLP-encoded BAL bytes, if present in the payload.
        bal: Option<Bytes>,
    },
    /// A block downloaded by the engine's block downloader.
    /// Contains header info needed for persistence and BAL resolution.
    DownloadedBlock {
        /// Block number.
        number: u64,
        /// Block hash.
        hash: B256,
        /// State root from the block header.
        state_root: B256,
        /// Parent block hash.
        parent_hash: B256,
    },
    /// The canonical head changed via `forkchoiceUpdated`.
    NewHead {
        /// Head block hash.
        head_hash: B256,
    },
}

/// Outcome reported by the orchestrator when snap sync completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapSyncOutcome {
    /// The block number that was synced to.
    pub synced_to: u64,
    /// Block hash of the synced-to block.
    pub block_hash: B256,
}

/// Errors that can occur during snap sync.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// A network request failed.
    #[error("network request failed: {0}")]
    Network(String),
    /// Database operation failed.
    #[error("database error: {0}")]
    Database(String),
    /// RLP decoding failed.
    #[error("RLP decode error: {0}")]
    RlpDecode(String),
    /// BAL verification failed (hash mismatch).
    #[error("BAL verification failed for block {block}: expected {expected}, got {got}")]
    BalVerification {
        /// Block number.
        block: u64,
        /// Expected hash from the header.
        expected: B256,
        /// Computed hash from the BAL bytes.
        got: B256,
    },
    /// Header not found.
    #[error("header not found for block {0}")]
    MissingHeader(u64),
    /// Block hash not found.
    #[error("block hash not found for block {0}")]
    MissingBlockHash(u64),
    /// State root mismatch after sync.
    #[error("state root mismatch at block {block}: expected {expected}, computed {computed}")]
    StateRootMismatch {
        /// Block number.
        block: u64,
        /// Expected state root from header.
        expected: B256,
        /// Computed state root.
        computed: B256,
    },
    /// Event channel closed unexpectedly.
    #[error("event channel closed")]
    ChannelClosed,
    /// BAL not available for a required block.
    #[error("BAL not available for block {0}")]
    MissingBal(u64),
}
