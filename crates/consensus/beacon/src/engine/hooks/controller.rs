use crate::hooks::{Hook, HookAction, HookArguments, HookDependencies, HookError, Hooks};
use std::task::{Context, Poll};
use tracing::debug;

/// Manages hooks under the control of the engine.
///
/// This type polls the initialized hooks one by one, respecting the dependencies (such as DB write
/// access that enforces running at most one such hook).
pub(crate) struct HooksController {
    /// Collection of hooks.
    ///
    /// [`Option::None`] means that the hook is taken, and will be returned upon completion.
    /// In the current implementation, it means that it was moved to `running_hook_with_db_write`.
    hooks: Vec<Option<Box<dyn Hook>>>,
    /// Next hook index to poll.
    next_hook_idx: usize,
    /// Currently running hook with DB write access, if any.
    running_hook_with_db_write: Option<(usize, Box<dyn Hook>)>,
}

impl HooksController {
    /// Creates a new [`HooksController`].
    pub(crate) fn new(hooks: Hooks) -> Self {
        Self {
            hooks: hooks.inner.into_iter().map(Some).collect(),
            next_hook_idx: 0,
            running_hook_with_db_write: None,
        }
    }

    /// Polls currently running hook with DB write access, if any.
    ///
    /// Returns [`Poll::Ready`] if currently running hook with DB write access returned
    /// an [event][`crate::hooks::HookEvent`] that resulted in [action][`HookAction`] or error.
    ///
    /// Returns [`Poll::Pending`] in all other cases:
    /// 1. No hook with DB write access is running.
    /// 2. Currently running hook with DB write access returned [`Poll::Pending`] on polling.
    /// 3. Currently running hook with DB write access returned [`Poll::Ready`] on polling, but
    ///    [`Hook::on_event`] returned [`Option::None`], so there's no action to act upon.
    pub(crate) fn poll_running_hook_with_db_write(
        &mut self,
        cx: &mut Context<'_>,
        args: HookArguments,
    ) -> Poll<Result<HookAction, HookError>> {
        let Some((hook_idx, mut hook)) = self.running_hook_with_db_write.take() else {
            return Poll::Pending
        };

        match hook.poll(cx, args) {
            Poll::Ready(event) => {
                let event_name = format!("{event:?}");
                let finished = event.is_finished();
                let action = hook.on_event(event)?;

                debug!(
                    target: "consensus::engine::hooks",
                    ?hook,
                    ?action,
                    event = %event_name,
                    "Polled running hook with db write access"
                );

                if !finished {
                    self.running_hook_with_db_write = Some((hook_idx, hook));
                } else {
                    self.hooks[hook_idx] = Some(hook);
                }

                if let Some(action) = action {
                    return Poll::Ready(Ok(action))
                }
            }
            Poll::Pending => {
                self.running_hook_with_db_write = Some((hook_idx, hook));
            }
        }

        Poll::Pending
    }

    /// Polls next hook from the collection.
    ///
    /// Returns [`Poll::Ready`] if next hook returned an [event][`crate::hooks::HookEvent`] that
    /// resulted in [action][`HookAction`].
    ///
    /// Returns [`Poll::Pending`] in all other cases:
    /// 1. Next hook is [`Option::None`], i.e. taken, meaning it's currently running and has a DB
    ///    write access.
    /// 2. Next hook needs a DB write access, but either there's another hook with DB write access
    ///    running, or [active_dependencies][`HookDependencies`] passed into arguments has `db_write
    ///    = true`.
    /// 3. Next hook returned [`Poll::Pending`] on polling.
    /// 4. Next hook returned [`Poll::Ready`] on polling, but [`Hook::on_event`] returned
    ///    [`Option::None`], so there's no action to act upon.
    pub(crate) fn poll_next_hook(
        &mut self,
        cx: &mut Context<'_>,
        args: HookArguments,
        active_dependencies: HookDependencies,
    ) -> Poll<Result<HookAction, HookError>> {
        let hook_idx = self.next_hook_idx % self.hooks.len();
        self.next_hook_idx = hook_idx + 1;

        // SAFETY: bounds are respected in the modulo above
        let Some(mut hook) = self.hooks[hook_idx].take() else { return Poll::Pending };

        // Hook with DB write dependency is not allowed to run due to already
        // running hook with DB write dependency or active DB write according to passed dependencies
        if hook.dependencies().db_write &&
            (self.running_hook_with_db_write.is_some() || active_dependencies.db_write)
        {
            return Poll::Pending
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
                return Poll::Ready(Ok(action))
            }
        } else {
            self.hooks[hook_idx] = Some(hook);
        }

        Poll::Pending
    }

    /// Returns `true` if there's a hook with DB write access running.
    pub(crate) fn is_hook_with_db_write_running(&self) -> bool {
        self.running_hook_with_db_write.is_some()
    }
}
