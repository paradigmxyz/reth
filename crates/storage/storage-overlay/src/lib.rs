//! State trie overlay construction and caching.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod builder;
pub use builder::*;

mod changeset_cache;
pub(crate) use changeset_cache::ChangesetCache;

mod changesets;

mod context;
pub use context::{OverlayContext, OverlayView};

mod manager_metrics;

mod multiproof_calculator;

mod overlay;
pub use overlay::{ExecutionOverlay, OverlayError};

mod provider;
pub use provider::*;

#[cfg(test)]
mod test_utils;
