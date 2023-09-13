use crate::hooks::{Hook, HookAction, HookArguments, HookDependencies, HookError, Hooks};
use std::task::{Context, Poll};
use tracing::debug;

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
        active_dependencies: HookDependencies,
    ) -> Result<Poll<HookAction>, HookError> {
        let hook_idx = self.hook_idx % self.hooks.len();
        self.hook_idx = hook_idx + 1;

        // SAFETY: bounds are respected in the modulo above
        let Some(mut hook) = self.hooks[hook_idx].take() else { return Ok(Poll::Pending) };

        // Hook with DB write dependency is not allowed to run due to already
        // running hook with DB write dependency or active DB write according to passed dependencies
        if hook.dependencies().db_write &&
            (self.running_hook_with_db_write.is_some() || active_dependencies.db_write)
        {
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
