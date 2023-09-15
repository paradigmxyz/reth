use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;
use std::{
    fmt::Debug,
    task::{Context, Poll},
};

mod controller;
pub(crate) use controller::HooksController;

mod prune;
pub use prune::PruneHook;

/// Collection of [hooks][`Hook`].
#[derive(Default)]
pub struct Hooks {
    inner: Vec<Box<dyn Hook>>,
}

impl Hooks {
    /// Creates a new empty collection of [hooks][`Hook`].
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Adds a new [hook][`Hook`] to the collection.
    pub fn add<H: Hook>(&mut self, hook: H) {
        self.inner.push(Box::new(hook))
    }
}

/// Hook that will be run during the main loop of
/// [consensus engine][`crate::engine::BeaconConsensusEngine`].
pub trait Hook: Send + Sync + 'static {
    /// Returns a human-readable name for the hook.
    fn name(&self) -> &'static str;

    /// Advances the hook execution, emitting an [event][`HookEvent`] and an optional
    /// [action][`HookAction`].
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        args: HookArguments,
    ) -> Poll<(HookEvent, Option<HookAction>)>;

    /// Returns [db access level][`HookDBAccessLevel`] the hook needs.
    fn db_access_level(&self) -> HookDBAccessLevel;
}

/// Arguments passed to the [hook polling function][`Hook::poll`].
#[derive(Copy, Clone, Debug)]
pub struct HookArguments {
    /// Tip block number.
    pub tip_block_number: BlockNumber,
}

/// An event emitted when [hook][`Hook`] is polled.
#[derive(Debug)]
pub enum HookEvent {
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
    Finished(Result<(), HookError>),
}

impl HookEvent {
    /// Returns `true` if the event is [`HookEvent::Started`].
    pub fn is_started(&self) -> bool {
        matches!(self, Self::Started)
    }

    /// Returns `true` if the event is [`HookEvent::Finished`].
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Finished(_))
    }
}

/// An action that the caller of [hook][`Hook`] should act upon.
#[derive(Debug, Copy, Clone)]
pub enum HookAction {
    /// Notify about a [SyncState] update.
    UpdateSyncState(SyncState),
    /// Read the last relevant canonical hashes from the database and update the block indices of
    /// the blockchain tree.
    RestoreCanonicalHashes,
}

/// An error returned by [hook][`Hook`].
#[derive(Debug, thiserror::Error)]
pub enum HookError {
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
pub enum HookDBAccessLevel {
    /// Read-only database access.
    ReadOnly,
    /// Read-write database access.
    ReadWrite,
}

impl HookDBAccessLevel {
    /// Returns `true` if the hook needs read-only access to the database.
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
    }

    /// Returns `true` if the hook needs read-write access to the database.
    pub fn is_read_write(&self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}
