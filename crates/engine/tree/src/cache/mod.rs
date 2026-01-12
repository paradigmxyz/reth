//! Caching components for the engine tree.
//!
//! This module provides in-memory caching infrastructure for engine tree components,
//! optimizing performance by avoiding unnecessary database reads and writes.
//!
//! ## Changeset Cache
//!
//! The [`ChangesetCache`](changeset_cache::ChangesetCache) provides in-memory storage
//! for trie changesets, which represent the old values of trie nodes before a block
//! was applied. This enables:
//!
//! - Fast reorg support without database reads
//! - Elimination of changeset database writes during live sync
//! - Automatic eviction based on configurable capacity

pub mod changeset_cache;

pub use changeset_cache::{ChangesetCache, ChangesetCacheHandle};
