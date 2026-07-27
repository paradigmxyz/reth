//! State trie overlay construction and caching.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod builder;
pub use builder::*;

mod changeset_cache;
pub use changeset_cache::*;

mod manager;
pub use manager::*;
