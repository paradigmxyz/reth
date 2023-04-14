//! Traits used when interacting with the sync status of the network.

/// A type that provides information about whether the node is currently syncing and the network is
/// currently serving syncing related requests.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SyncStateProvider: Send + Sync {
    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;
}

/// An updater for updating the [SyncState] of the network.
///
/// The node is either syncing, or it is idle.
/// While syncing, the node will download data from the network and process it. The processing
/// consists of several stages, like recovering senders, executing the blocks and indexing.
/// Eventually the node reaches the `Finish` stage and will transition to [`SyncState::Idle`], it
/// which point the node is considered fully synced.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SyncStateUpdater: SyncStateProvider {
    /// Notifies about an [SyncState] update.
    fn update_sync_state(&self, state: SyncState);
}

/// The state the network is currently in when it comes to synchronization.
#[derive(Clone, Eq, PartialEq, Debug)]
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

/// A [SyncStateUpdater] implementation that does nothing.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopSyncStateUpdate;

impl SyncStateProvider for NoopSyncStateUpdate {
    fn is_syncing(&self) -> bool {
        false
    }
}

impl SyncStateUpdater for NoopSyncStateUpdate {
    fn update_sync_state(&self, _state: SyncState) {}
}
