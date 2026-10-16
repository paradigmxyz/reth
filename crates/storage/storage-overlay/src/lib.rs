//! State trie overlay construction and caching.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod builder;
pub use builder::*;

mod changeset_cache;
pub(crate) use changeset_cache::ChangesetCache;

mod manager;
pub use manager::*;

mod manager_metrics;

mod provider;
pub use provider::*;
