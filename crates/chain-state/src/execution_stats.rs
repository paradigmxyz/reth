//! Execution timing statistics for slow block logging.
//!
//! This module provides types for collecting and passing execution timing statistics
//! through the block processing pipeline, enabling unified slow block logging after
//! database commit.

use std::time::Duration;

use alloy_primitives::B256;

/// Statistics collected during block execution for cross-client performance analysis.
///
/// These statistics are populated during block validation and carried through to
/// persistence, where they are used to emit a single unified log entry that includes
/// complete timing information (including commit time).
#[derive(Debug, Clone, Default)]
pub struct ExecutionTimingStats {
    /// Block number
    pub block_number: u64,
    /// Block hash as formatted string (0x-prefixed hex)
    pub block_hash: B256,
    /// Total gas used by the block
    pub gas_used: u64,
    /// Number of transactions in the block
    pub tx_count: usize,
    /// Time spent executing transactions (milliseconds)
    pub execution_duration: Duration,
    /// Time spent reading state during execution (milliseconds)
    pub state_read_duration: Duration,
    /// Time spent computing state root hash (milliseconds)
    pub state_hash_duration: Duration,
    /// Number of accounts read during execution
    pub accounts_read: usize,
    /// Number of storage slots read (SLOAD operations)
    pub storage_read: usize,
    /// Number of code reads (EXTCODE* operations)
    pub code_read: usize,
    /// Total bytes of code read
    pub code_bytes_read: usize,
    /// Number of accounts changed (balance/nonce updates)
    pub accounts_changed: usize,
    /// Number of accounts deleted (SELFDESTRUCT)
    pub accounts_deleted: usize,
    /// Number of storage slots changed (SSTORE operations)
    pub storage_slots_changed: usize,
    /// Number of storage slots deleted (set to zero)
    pub storage_slots_deleted: usize,
    /// Number of bytecodes created/changed (contract deployments)
    pub bytecodes_changed: usize,
    /// Total bytes of code written
    pub code_bytes_written: usize,
    /// Number of EIP-7702 delegations set
    pub eip7702_delegations_set: usize,
    /// Number of EIP-7702 delegations cleared
    pub eip7702_delegations_cleared: usize,
    /// Account cache hits
    pub account_cache_hits: usize,
    /// Account cache misses
    pub account_cache_misses: usize,
    /// Storage cache hits
    pub storage_cache_hits: usize,
    /// Storage cache misses
    pub storage_cache_misses: usize,
    /// Code cache hits
    pub code_cache_hits: usize,
    /// Code cache misses
    pub code_cache_misses: usize,
}
