use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;
use std::{
    fmt::Debug,
    task::{Context, Poll},
};

mod controller;
pub(crate) use controller::EngineHooksController;

mod prune;
pub use prune::PruneHook;

/// Collection of [engine hooks][`EngineHook`].
#[derive(Default)]
pub struct EngineHooks {
    inner: Vec<Box<dyn EngineHook>>,
}

impl EngineHooks {
    /// Creates a new empty collection of [engine hooks][`EngineHook`].
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Adds a new [engine hook][`EngineHook`] to the collection.
    pub fn add<H: EngineHook>(&mut self, hook: H) {
        self.inner.push(Box::new(hook))
    }
}

/// Hook that will be run during the main loop of
/// [consensus engine][`crate::engine::BeaconConsensusEngine`].
pub trait EngineHook: Send + Sync + 'static {
    /// Returns a human-readable name for the hook.
    fn name(&self) -> &'static str;

    /// Advances the hook execution, emitting an [event][`EngineHookEvent`] and an optional
    /// [action][`EngineHookAction`].
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        ctx: EngineContext,
    ) -> Poll<(EngineHookEvent, Option<EngineHookAction>)>;

    /// Returns [db access level][`EngineHookDBAccessLevel`] the hook needs.
    fn db_access_level(&self) -> EngineHookDBAccessLevel;
}

/// Engine context passed to the [hook polling function][`EngineHook::poll`].
#[derive(Copy, Clone, Debug)]
pub struct EngineContext {
    /// Tip block number.
    pub tip_block_number: BlockNumber,
}

/// An event emitted when [hook][`EngineHook`] is polled.
#[derive(Debug)]
pub enum EngineHookEvent {
    /// Hook is not ready.
    ///
    /// If this is returned, the hook is idle.
    NotReady,
    /// Hook started.
    ///
    /// If this is returned, the hook is running.
    Started,
    /// Hook finished.
    ///
    /// If this is returned, the hook is idle.
    Finished(Result<(), EngineHookError>),
}

impl EngineHookEvent {
    /// Returns `true` if the event is [`EngineHookEvent::Started`].
    pub fn is_started(&self) -> bool {
        matches!(self, Self::Started)
    }

    /// Returns `true` if the event is [`EngineHookEvent::Finished`].
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Finished(_))
    }
}

/// An action that the caller of [hook][`EngineHook`] should act upon.
#[derive(Debug, Copy, Clone)]
pub enum EngineHookAction {
    /// Notify about a [SyncState] update.
    UpdateSyncState(SyncState),
    /// Connect blocks buffered during the hook execution to canonical hashes.
    ConnectBufferedBlocks,
}

/// An error returned by [hook][`EngineHook`].
#[derive(Debug, thiserror::Error)]
pub enum EngineHookError {
    /// Hook channel closed.
    #[error("Hook channel closed")]
    ChannelClosed,
    /// Common error. Wrapper around [reth_interfaces::Error].
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
    /// An internal error occurred.
    #[error("Internal hook error occurred.")]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Level of database access the hook needs for execution.
pub enum EngineHookDBAccessLevel {
    /// Read-only database access.
    ReadOnly,
    /// Read-write database access.
    ReadWrite,
}

impl EngineHookDBAccessLevel {
    /// Returns `true` if the hook needs read-only access to the database.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }

    /// Returns `true` if the hook needs read-write access to the database.
    pub fn is_read_write(&self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}
