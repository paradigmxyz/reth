//! Traits used when interacting with the sync status of the network.

use reth_primitives::Head;

/// A type that provides information about whether the node is currently syncing and the network is
/// currently serving syncing related requests.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SyncStateProvider: Send + Sync {
    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns `true` if the network is undergoing an initial (pipeline) sync.
    fn is_initially_syncing(&self) -> bool;
}

/// An updater for updating the [SyncState] and status of the network.
///
/// The node is either syncing, or it is idle.
/// While syncing, the node will download data from the network and process it. The processing
/// consists of several stages, like recovering senders, executing the blocks and indexing.
/// Eventually the node reaches the `Finish` stage and will transition to [`SyncState::Idle`], it
/// which point the node is considered fully synced.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait NetworkSyncUpdater: std::fmt::Debug + Send + Sync + 'static {
    /// Notifies about a [SyncState] update.
    fn update_sync_state(&self, state: SyncState);

    /// Updates the status of the p2p node
    fn update_status(&self, head: Head);
}

/// The state the network is currently in when it comes to synchronization.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum SyncState {
    /// Node sync is complete.
    ///
    /// The network just serves requests to keep up of the chain.
    Idle,
    /// Network is syncing
    Syncing,
}

impl SyncState {
    /// Whether the node is currently syncing.
    ///
    /// Note: this does not include keep-up sync when the state is idle.
    pub fn is_syncing(&self) -> bool {
        !matches!(self, SyncState::Idle)
    }
}

/// A [NetworkSyncUpdater] implementation that does nothing.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct NoopSyncStateUpdater;

impl SyncStateProvider for NoopSyncStateUpdater {
    fn is_syncing(&self) -> bool {
        false
    }
    fn is_initially_syncing(&self) -> bool {
        false
    }
}

impl NetworkSyncUpdater for NoopSyncStateUpdater {
    fn update_sync_state(&self, _state: SyncState) {}
    fn update_status(&self, _: Head) {}
}
