//! Abstractions for chain syncing.

/// Abstraction over how the chain syncs, manages downloads etc.
///
/// This is done to hide internals.
pub trait StateSync: Send + Sync + 'static {}
