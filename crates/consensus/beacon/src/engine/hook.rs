use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;
use std::task::{Context, Poll};

pub trait Hook: Send + Sync + 'static {
    fn poll(&mut self, cx: &mut Context<'_>, args: HookArguments) -> Poll<HookEvent>;

    fn on_event(&mut self, event: HookEvent) -> Result<Option<HookAction>, HookError>;

    fn capabilities(&self) -> HookCapabilities;
}

pub struct HookArguments {
    pub tip_block_number: BlockNumber,
}

pub enum HookEvent {
    /// Hook is not ready.
    NotReady,
    /// Hook started with tip block number.
    Started(BlockNumber),
    /// Hook finished.
    ///
    /// If this is returned, the hook is idle.
    Finished(Result<(), HookError>),
}

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

pub struct HookCapabilities {
    pub db_write: bool,
}
