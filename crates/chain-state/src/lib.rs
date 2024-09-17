//! Reth state related types and functionality.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod in_memory;
pub use in_memory::*;

mod chain_info;
pub use chain_info::ChainInfoTracker;

mod notifications;
pub use notifications::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotificationStream,
    CanonStateNotifications, CanonStateSubscriptions, ForkChoiceNotifications, ForkChoiceStream,
    ForkChoiceSubscriptions,
};

mod memory_overlay;
pub use memory_overlay::MemoryOverlayStateProvider;

#[cfg(any(test, feature = "test-utils"))]
/// Common test helpers
pub mod test_utils;
