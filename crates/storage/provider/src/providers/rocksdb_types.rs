//! Shared types for `RocksDB` provider.
//!
//! These types are used by both the real `RocksDB` implementation and the stub.

/// Outcome of pruning a history shard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneShardOutcome {
    /// Shard was deleted entirely.
    Deleted,
    /// Shard was updated with filtered block numbers.
    Updated,
    /// Shard was unchanged (no blocks <= `to_block`).
    Unchanged,
}
