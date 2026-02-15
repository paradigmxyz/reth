//! Reth state related types and functionality.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod execution_stats;
pub use execution_stats::ExecutionTimingStats;

mod in_memory;
pub use in_memory::*;

mod deferred_trie;
pub use deferred_trie::*;

mod lazy_overlay;
pub use lazy_overlay::*;

mod noop;

mod chain_info;
pub use chain_info::ChainInfoTracker;

mod notifications;
pub use notifications::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotificationStream,
    CanonStateNotifications, CanonStateSubscriptions, ForkChoiceNotifications, ForkChoiceStream,
    ForkChoiceSubscriptions, PersistedBlockNotifications, PersistedBlockSubscriptions,
    WatchValueStream,
};

mod memory_overlay;
pub use memory_overlay::{MemoryOverlayStateProvider, MemoryOverlayStateProviderRef};

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers
pub mod test_utils;

// todo: remove when generic data prim integration complete
pub use reth_ethereum_primitives::EthPrimitives;
