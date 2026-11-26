//! State trie overlay construction and caching.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod builder;
pub use builder::*;

mod changeset_cache;
pub(crate) use changeset_cache::ChangesetCache;
pub use changeset_cache::{compute_block_trie_updates, get_or_compute_cached_changesets_range};

mod manager;
pub use manager::*;

mod manager_metrics;

mod multiproof_calculator;

mod provider;
pub use provider::*;

#[cfg(test)]
mod test_utils;
