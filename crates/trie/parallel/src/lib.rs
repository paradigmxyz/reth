//! Implementation of exotic state root computation approaches.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod storage_root_targets;
pub use storage_root_targets::StorageRootTargets;

/// Parallel trie calculation stats.
pub mod stats;

/// Implementation of async state root computation.
#[cfg(feature = "async")]
pub mod async_root;

/// Implementation of parallel state root computation.
#[cfg(feature = "parallel")]
pub mod parallel_root;

/// Parallel state root metrics.
#[cfg(feature = "metrics")]
pub mod metrics;
