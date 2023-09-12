use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;
use std::task::{Context, Poll};

pub trait Hook: Send + Sync + 'static {
    fn name(&self) -> &'static str {
        return std::any::type_name::<Self>()
    }
    fn poll(&mut self, cx: &mut Context<'_>, args: HookArguments) -> Poll<HookEvent>;

    fn on_event(&mut self, event: HookEvent) -> Result<Option<HookAction>, HookError>;

    fn dependencies(&self) -> HookDependencies;
}

impl std::fmt::Debug for dyn Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

pub struct HookArguments {
    pub tip_block_number: BlockNumber,
}

#[derive(Debug)]
pub enum HookEvent {
    /// Hook is not ready.
    ///
    /// If this is returned, the hook is idle.
    NotReady,
    /// Hook started with tip block number.
    ///
    /// If this is returned, the hook is running.
    Started(BlockNumber),
    /// Hook finished.
    ///
    /// If this is returned, the hook is idle.
    Finished(Result<(), HookError>),
}

impl HookEvent {
    pub fn is_started(&self) -> bool {
        matches!(self, Self::Started(_))
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Finished(_))
    }
}

#[derive(Debug)]
pub enum HookAction {
    UpdateSyncState(SyncState),
    RestoreCanonicalHashes,
}

#[derive(Debug, thiserror::Error)]
pub enum HookError {
    /// Hook channel closed.
    #[error("Hook channel closed")]
    ChannelClosed,
    #[error(transparent)]
    Common(#[from] reth_interfaces::Error),
    #[error("Internal hook error occurred.")]
    Internal(#[from] Box<dyn std::error::Error + Send>),
}

pub struct HookDependencies {
    pub db_write: bool,
    pub pipeline_idle: bool,
}
