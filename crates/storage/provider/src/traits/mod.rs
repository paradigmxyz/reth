//! Collection of common provider traits.

// Re-export all the traits
pub use reth_storage_api::*;

// Re-export for convenience
pub use reth_evm::provider::EvmEnvProvider;

mod block;
pub use block::*;

mod chain_info;
pub use chain_info::CanonChainTracker;

mod header_sync_gap;
pub use header_sync_gap::{HeaderSyncGap, HeaderSyncGapProvider, HeaderSyncMode};

mod state;
pub use state::StateWriter;

mod chain;
pub use chain::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotificationStream,
    CanonStateNotifications, CanonStateSubscriptions,
};

mod spec;
pub use spec::ChainSpecProvider;

mod hashing;
pub use hashing::HashingWriter;

mod history;
pub use history::HistoryWriter;

mod database_provider;
pub use database_provider::DatabaseProviderFactory;

mod static_file_provider;
pub use static_file_provider::StaticFileProviderFactory;

mod stats;
pub use stats::StatsReader;

mod full;
pub use full::FullProvider;

mod tree_viewer;
pub use tree_viewer::TreeViewer;
