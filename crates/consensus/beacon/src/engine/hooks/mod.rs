use reth_interfaces::sync::SyncState;
use reth_primitives::BlockNumber;
use std::task::{Context, Poll};
use tracing::debug;

mod prune;
pub use prune::EnginePruneController;

#[derive(Default)]
pub struct Hooks {
    inner: Vec<Box<dyn Hook>>,
}

impl Hooks {
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    pub fn add<H: Hook>(&mut self, hook: H) {
        self.inner.push(Box::new(hook))
    }
}

pub trait Hook: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn poll(&mut self, cx: &mut Context<'_>, args: HookArguments) -> Poll<HookEvent>;

    fn on_event(&mut self, event: HookEvent) -> Result<Option<HookAction>, HookError>;

    fn dependencies(&self) -> HookDependencies;
}

impl std::fmt::Debug for dyn Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[derive(Copy, Clone, Debug)]
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

#[derive(Debug, Copy, Clone)]
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
    Internal(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub struct HookDependencies {
    pub db_write: bool,
    pub pipeline_idle: bool,
}

pub(crate) struct HooksController {
    hooks: Vec<Option<Box<dyn Hook>>>,
    hook_idx: usize,
    running_hook_with_db_write: Option<(usize, Box<dyn Hook>)>,
}

impl HooksController {
    pub(crate) fn new(hooks: Hooks) -> Self {
        Self {
            hooks: hooks.inner.into_iter().map(Some).collect(),
            hook_idx: 0,
            running_hook_with_db_write: None,
        }
    }
    pub(crate) fn poll_running_hook_with_db_write(
        &mut self,
        cx: &mut Context<'_>,
        args: HookArguments,
    ) -> Result<Poll<HookAction>, HookError> {
        if let Some((hook_idx, mut hook)) = self.running_hook_with_db_write.take() {
            match hook.poll(cx, args) {
                Poll::Ready(event) => {
                    let event_name = format!("{event:?}");
                    let finished = event.is_finished();
                    let action = hook.on_event(event)?;

                    debug!(target: "consensus::engine::hooks", ?hook, ?action, event = %event_name, "Polled running hook with db write");

                    if !finished {
                        self.running_hook_with_db_write = Some((hook_idx, hook));
                    } else {
                        self.hooks[hook_idx] = Some(hook);
                    }

                    if let Some(action) = action {
                        return Ok(Poll::Ready(action))
                    }
                }
                Poll::Pending => {
                    self.running_hook_with_db_write = Some((hook_idx, hook));
                }
            }
        }

        Ok(Poll::Pending)
    }

    pub(crate) fn poll_next_hook(
        &mut self,
        cx: &mut Context<'_>,
        args: HookArguments,
        is_pipeline_active: bool,
    ) -> Result<Poll<HookAction>, HookError> {
        let hook_idx = self.hook_idx % self.hooks.len();
        self.hook_idx = hook_idx + 1;

        // SAFETY: bounds are respected in the modulo above
        let Some(mut hook) = self.hooks[hook_idx].take() else { return Ok(Poll::Pending) };

        // Hook with DB write dependency is not allowed to run due to already
        // running hook with DB write dependency.
        let db_write = hook.dependencies().db_write && self.running_hook_with_db_write.is_some();
        // Hook with idle pipeline dependency is not allowed to run due to pipeline
        // being active.
        let pipeline_idle = hook.dependencies().pipeline_idle && is_pipeline_active;

        let skip_hook = db_write || pipeline_idle;
        if skip_hook {
            return Ok(Poll::Pending)
        }

        if let Poll::Ready(event) = hook.poll(cx, args) {
            let event_name = format!("{event:?}");
            let started = event.is_started();
            let action = hook.on_event(event)?;

            debug!(target: "consensus::engine::hooks", ?hook, ?action, event = %event_name, "Polled next hook");

            if started && hook.dependencies().db_write {
                self.running_hook_with_db_write = Some((hook_idx, hook));
            } else {
                self.hooks[hook_idx] = Some(hook);
            }

            if let Some(action) = action {
                return Ok(Poll::Ready(action))
            }
        } else {
            self.hooks[hook_idx] = Some(hook);
        }

        Ok(Poll::Pending)
    }

    pub(crate) fn is_hook_with_db_write_running(&self) -> bool {
        self.running_hook_with_db_write.is_some()
    }
}
