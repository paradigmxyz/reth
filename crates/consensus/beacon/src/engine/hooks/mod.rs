use reth_interfaces::{RethError, RethResult};
use reth_primitives::BlockNumber;
use std::{
    fmt,
    task::{Context, Poll},
};

mod controller;
pub(crate) use controller::{EngineHooksController, PolledHook};

mod prune;
pub use prune::PruneHook;

mod static_file;
pub use static_file::StaticFileHook;

/// Collection of [engine hooks][`EngineHook`].
#[derive(Default)]
pub struct EngineHooks {
    inner: Vec<Box<dyn EngineHook>>,
}

impl fmt::Debug for EngineHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineHooks").field("inner", &self.inner.len()).finish()
    }
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

    /// Advances the hook execution, emitting an [event][`EngineHookEvent`].
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        ctx: EngineContext,
    ) -> Poll<RethResult<EngineHookEvent>>;

    /// Returns [db access level][`EngineHookDBAccessLevel`] the hook needs.
    fn db_access_level(&self) -> EngineHookDBAccessLevel;
}

/// Engine context passed to the [hook polling function][`EngineHook::poll`].
#[derive(Copy, Clone, Debug)]
pub struct EngineContext {
    /// Tip block number.
    pub tip_block_number: BlockNumber,
    /// Finalized block number, if known.
    pub finalized_block_number: Option<BlockNumber>,
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

/// An error returned by [hook][`EngineHook`].
#[derive(Debug, thiserror::Error)]
pub enum EngineHookError {
    /// Hook channel closed.
    #[error("hook channel closed")]
    ChannelClosed,
    /// Common error. Wrapper around [RethError].
    #[error(transparent)]
    Common(#[from] RethError),
    /// An internal error occurred.
    #[error(transparent)]
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Level of database access the hook needs for execution.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
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
