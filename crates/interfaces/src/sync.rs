//! Traits used when interacting with the sync status of the network.

use reth_primitives::BlockNumber;

/// A type that provides information about whether the node is currently syncing and the network is
/// currently serving syncing related requests.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SyncStateProvider: Send + Sync {
    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;
}

/// An updater for updating the [SyncState] of the network.
///
/// The chain sync pipeline consists of several sequential Stages, like the `HeaderStage` for
/// downloading bodies, or `ExecutionStage` for process all downloaded data.
///
/// Some stage transitions will result in an update of the [SyncState] of the network. For example,
/// the transition from a download stage (`Headers`, `Bodies`) to a processing stage (`Sender
/// Recovery`, `Execution`) marks a transition from [`SyncState::Downloading`] to
/// [`SyncState::Executing`]. Since the execution takes some time, after the first pass the node
/// will not be synced ([`SyncState::Idle`]) yet and instead transition back to download data, but
/// now with a higher `block_target`. This cycle will continue until the node has caught up with the
/// chain and will transition to [`SyncState::Idle`] sync.
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
    /// Network is syncing and downloading up to the `target_block`.
    ///
    /// This represents the headers and bodies stage.
    Downloading {
        /// The block to which the node is downloading state.
        target_block: BlockNumber,
    },
    /// All headers and bodies up to the `target_block` have been downloaded and are now being
    /// executed.
    ///
    /// This represents stages that execute/recover the downloaded data.
    Executing {
        /// The block to which the node executes downloaded state.
        target_block: BlockNumber,
    },
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
