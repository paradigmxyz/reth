use crate::hooks::{
    EngineContext, EngineHook, EngineHookAction, EngineHookDBAccessLevel, EngineHookError,
    EngineHookEvent, EngineHooks,
};
use std::{
    collections::VecDeque,
    task::{Context, Poll},
};
use tracing::debug;

#[derive(Debug)]
pub(crate) struct PolledHook {
    #[allow(unused)]
    pub(crate) name: &'static str,
    pub(crate) event: EngineHookEvent,
    pub(crate) action: Option<EngineHookAction>,
    pub(crate) db_access_level: EngineHookDBAccessLevel,
}

/// Manages hooks under the control of the engine.
///
/// This type polls the initialized hooks one by one, respecting the DB access level
/// (i.e. [crate::hooks::EngineHookDBAccessLevel::ReadWrite] that enforces running at most one such
/// hook).
pub(crate) struct EngineHooksController {
    /// Collection of hooks.
    ///
    /// Hooks might be removed from the collection, and returned upon completion.
    /// In the current implementation, it only happens when moved to `running_hook_with_db_write`.
    hooks: VecDeque<Box<dyn EngineHook>>,
    /// Currently running hook with DB write access, if any.
    running_hook_with_db_write: Option<Box<dyn EngineHook>>,
}

impl EngineHooksController {
    /// Creates a new [`EngineHooksController`].
    pub(crate) fn new(hooks: EngineHooks) -> Self {
        Self { hooks: hooks.inner.into(), running_hook_with_db_write: None }
    }

    /// Polls currently running hook with DB write access, if any.
    ///
    /// Returns [`Poll::Ready`] if currently running hook with DB write access returned
    /// an [event][`crate::hooks::EngineHookEvent`] that resulted in [action][`EngineHookAction`] or
    /// error.
    ///
    /// Returns [`Poll::Pending`] in all other cases:
    /// 1. No hook with DB write access is running.
    /// 2. Currently running hook with DB write access returned [`Poll::Pending`] on polling.
    /// 3. Currently running hook with DB write access returned [`Poll::Ready`] on polling, but no
    ///    action to act upon.
    pub(crate) fn poll_running_hook_with_db_write(
        &mut self,
        cx: &mut Context<'_>,
        args: EngineContext,
    ) -> Poll<Result<PolledHook, EngineHookError>> {
        let Some(mut hook) = self.running_hook_with_db_write.take() else { return Poll::Pending };

        match hook.poll(cx, args)? {
            Poll::Ready((event, action)) => {
                let result = PolledHook {
                    name: hook.name(),
                    event,
                    action,
                    db_access_level: hook.db_access_level(),
                };

                debug!(
                    target: "consensus::engine::hooks",
                    hook = hook.name(),
                    ?result,
                    "Polled running hook with db write access"
                );

                if !result.event.is_finished() {
                    self.running_hook_with_db_write = Some(hook);
                } else {
                    self.hooks.push_back(hook);
                }

                return Poll::Ready(Ok(result))
            }
            Poll::Pending => {
                self.running_hook_with_db_write = Some(hook);
            }
        }

        Poll::Pending
    }

    /// Polls next engine from the collection.
    ///
    /// Returns [`Poll::Ready`] if next hook returned an [event][`crate::hooks::EngineHookEvent`]
    /// that resulted in [action][`EngineHookAction`].
    ///
    /// Returns [`Poll::Pending`] in all other cases:
    /// 1. Next hook is [`Option::None`], i.e. taken, meaning it's currently running and has a DB
    ///    write access.
    /// 2. Next hook needs a DB write access, but either there's another hook with DB write access
    ///    running, or `db_write_active` passed into arguments is `true`.
    /// 3. Next hook returned [`Poll::Pending`] on polling.
    /// 4. Next hook returned [`Poll::Ready`] on polling, but no action to act upon.
    pub(crate) fn poll_next_hook(
        &mut self,
        cx: &mut Context<'_>,
        args: EngineContext,
        db_write_active: bool,
    ) -> Poll<Result<PolledHook, EngineHookError>> {
        let Some(mut hook) = self.hooks.pop_front() else { return Poll::Pending };

        let result = self.poll_next_hook_inner(cx, &mut hook, args, db_write_active);

        if matches!(
            result,
            Poll::Ready(Ok(PolledHook {
                event: EngineHookEvent::Started,
                db_access_level: EngineHookDBAccessLevel::ReadWrite,
                ..
            }))
        ) {
            // If a read-write hook started, set `running_hook_with_db_write` to it
            self.running_hook_with_db_write = Some(hook);
        } else {
            // Otherwise, push it back to the collection of hooks to poll it next time
            self.hooks.push_back(hook);
        }

        result
    }

    fn poll_next_hook_inner(
        &mut self,
        cx: &mut Context<'_>,
        hook: &mut Box<dyn EngineHook>,
        args: EngineContext,
        db_write_active: bool,
    ) -> Poll<Result<PolledHook, EngineHookError>> {
        // Hook with DB write access level is not allowed to run due to already running hook with DB
        // write access level or active DB write according to passed argument
        if hook.db_access_level().is_read_write() &&
            (self.running_hook_with_db_write.is_some() || db_write_active)
        {
            return Poll::Pending
        }

        if let Poll::Ready((event, action)) = hook.poll(cx, args)? {
            let result = PolledHook {
                name: hook.name(),
                event,
                action,
                db_access_level: hook.db_access_level(),
            };

            debug!(
                target: "consensus::engine::hooks",
                hook = hook.name(),
                ?result,
                "Polled next hook"
            );

            return Poll::Ready(Ok(result))
        }

        Poll::Pending
    }

    /// Returns `true` if there's a hook with DB write access running.
    pub(crate) fn is_hook_with_db_write_running(&self) -> bool {
        self.running_hook_with_db_write.is_some()
    }
}

#[cfg(test)]
mod tests {
    use crate::hooks::{
        EngineContext, EngineHook, EngineHookAction, EngineHookDBAccessLevel, EngineHookEvent,
        EngineHooks, EngineHooksController,
    };
    use futures::poll;
    use reth_interfaces::{RethError, RethResult};
    use std::{
        collections::VecDeque,
        future::poll_fn,
        task::{Context, Poll},
    };

    struct TestHook {
        results: VecDeque<RethResult<(EngineHookEvent, Option<EngineHookAction>)>>,
        name: &'static str,
        access_level: EngineHookDBAccessLevel,
    }

    impl TestHook {
        fn new_ro(name: &'static str) -> Self {
            Self {
                results: Default::default(),
                name,
                access_level: EngineHookDBAccessLevel::ReadOnly,
            }
        }
        fn new_rw(name: &'static str) -> Self {
            Self {
                results: Default::default(),
                name,
                access_level: EngineHookDBAccessLevel::ReadWrite,
            }
        }

        fn add_result(&mut self, result: RethResult<(EngineHookEvent, Option<EngineHookAction>)>) {
            self.results.push_back(result);
        }
    }

    impl EngineHook for TestHook {
        fn name(&self) -> &'static str {
            self.name
        }

        fn poll(
            &mut self,
            _cx: &mut Context<'_>,
            _ctx: EngineContext,
        ) -> Poll<RethResult<(EngineHookEvent, Option<EngineHookAction>)>> {
            self.results.pop_front().map_or(Poll::Pending, Poll::Ready)
        }

        fn db_access_level(&self) -> EngineHookDBAccessLevel {
            self.access_level
        }
    }

    #[tokio::test]
    async fn poll_running_hook_with_db_write() {
        let mut controller = EngineHooksController::new(EngineHooks::new());

        let context = EngineContext { tip_block_number: 2, finalized_block_number: Some(1) };

        // No currently running hook with DB write access is set
        let result = poll!(poll_fn(|cx| controller.poll_running_hook_with_db_write(cx, context)));
        assert!(result.is_pending());

        // Currently running hook with DB write access returned `Pending` on polling
        controller.running_hook_with_db_write = Some(Box::new(TestHook::new_rw("read-write")));

        let result = poll!(poll_fn(|cx| controller.poll_running_hook_with_db_write(cx, context)));
        assert!(result.is_pending());

        // Currently running hook with DB write access returned `Ready` on polling, but didn't
        // return `EngineHookEvent::Finished` yet.
        // Currently running hooks with DB write should still be set.
        let mut hook = TestHook::new_rw("read-write");
        hook.add_result(Ok((EngineHookEvent::Started, None)));
        controller.running_hook_with_db_write = Some(Box::new(hook));

        let result = poll!(poll_fn(|cx| controller.poll_running_hook_with_db_write(cx, context)));
        assert_eq!(
            result.map(|result| {
                let polled_hook = result.unwrap();
                polled_hook.event.is_started() &&
                    polled_hook.action.is_none() &&
                    polled_hook.db_access_level.is_read_write()
            }),
            Poll::Ready(true)
        );
        assert!(controller.running_hook_with_db_write.is_some());
        assert!(controller.hooks.is_empty());

        // Currently running hook with DB write access returned `Ready` on polling and
        // `EngineHookEvent::Finished` inside.
        // Currently running hooks with DB write should be moved to collection of hooks.
        let mut hook = TestHook::new_rw("read-write");
        hook.add_result(Ok((EngineHookEvent::Finished(Ok(())), None)));
        controller.running_hook_with_db_write = Some(Box::new(hook));

        let result = poll!(poll_fn(|cx| controller.poll_running_hook_with_db_write(cx, context)));
        assert_eq!(
            result.map(|result| {
                let polled_hook = result.unwrap();
                polled_hook.event.is_finished() &&
                    polled_hook.action.is_none() &&
                    polled_hook.db_access_level.is_read_write()
            }),
            Poll::Ready(true)
        );
        assert!(controller.running_hook_with_db_write.is_none());
        assert!(controller.hooks.pop_front().is_some());
    }

    #[tokio::test]
    async fn poll_next_hook_db_write_active() {
        let context = EngineContext { tip_block_number: 2, finalized_block_number: Some(1) };

        let mut hook_rw = TestHook::new_rw("read-write");
        hook_rw.add_result(Ok((EngineHookEvent::Started, None)));

        let hook_ro_name = "read-only";
        let mut hook_ro = TestHook::new_ro(hook_ro_name);
        hook_ro.add_result(Ok((EngineHookEvent::Started, None)));

        let mut hooks = EngineHooks::new();
        hooks.add(hook_rw);
        hooks.add(hook_ro);
        let mut controller = EngineHooksController::new(hooks);

        // Read-write hook can't be polled when external DB write is active
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, true)));
        assert!(result.is_pending());
        assert!(controller.running_hook_with_db_write.is_none());

        // Read-only hook can be polled when external DB write is active
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, true)));
        assert_eq!(
            result.map(|result| {
                let polled_hook = result.unwrap();
                polled_hook.name == hook_ro_name &&
                    polled_hook.event.is_started() &&
                    polled_hook.action.is_none() &&
                    polled_hook.db_access_level.is_read_only()
            }),
            Poll::Ready(true)
        );
    }

    #[tokio::test]
    async fn poll_next_hook_db_write_inactive() {
        let context = EngineContext { tip_block_number: 2, finalized_block_number: Some(1) };

        let hook_rw_1_name = "read-write-1";
        let mut hook_rw_1 = TestHook::new_rw(hook_rw_1_name);
        hook_rw_1.add_result(Ok((EngineHookEvent::Started, None)));

        let hook_rw_2_name = "read-write-2";
        let mut hook_rw_2 = TestHook::new_rw(hook_rw_2_name);
        hook_rw_2.add_result(Ok((EngineHookEvent::Started, None)));

        let hook_ro_name = "read-only";
        let mut hook_ro = TestHook::new_ro(hook_ro_name);
        hook_ro.add_result(Ok((EngineHookEvent::Started, None)));
        hook_ro.add_result(Err(RethError::Custom("something went wrong".to_string())));

        let mut hooks = EngineHooks::new();
        hooks.add(hook_rw_1);
        hooks.add(hook_rw_2);
        hooks.add(hook_ro);

        let mut controller = EngineHooksController::new(hooks);
        let hooks_len = controller.hooks.len();

        // Read-write hook can be polled because external DB write is not active
        assert_eq!(controller.hooks.front().map(|hook| hook.name()), Some(hook_rw_1_name));
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, false)));
        assert_eq!(
            result.map(|result| {
                let polled_hook = result.unwrap();
                polled_hook.name == hook_rw_1_name &&
                    polled_hook.event.is_started() &&
                    polled_hook.action.is_none() &&
                    polled_hook.db_access_level.is_read_write()
            }),
            Poll::Ready(true)
        );
        assert_eq!(
            controller.running_hook_with_db_write.as_ref().map(|hook| hook.name()),
            Some(hook_rw_1_name)
        );

        // Read-write hook cannot be polled because another read-write hook is running
        assert_eq!(controller.hooks.front().map(|hook| hook.name()), Some(hook_rw_2_name));
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, false)));
        assert!(result.is_pending());

        // Read-only hook can be polled in parallel with already running read-write hook
        assert_eq!(controller.hooks.front().map(|hook| hook.name()), Some(hook_ro_name));
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, false)));
        assert_eq!(
            result.map(|result| {
                let polled_hook = result.unwrap();
                polled_hook.name == hook_ro_name &&
                    polled_hook.event.is_started() &&
                    polled_hook.action.is_none() &&
                    polled_hook.db_access_level.is_read_only()
            }),
            Poll::Ready(true)
        );

        // Read-write hook still cannot be polled because another read-write hook is running
        assert_eq!(controller.hooks.front().map(|hook| hook.name()), Some(hook_rw_2_name));
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, false)));
        assert!(result.is_pending());

        // Read-only hook has finished with error
        assert_eq!(controller.hooks.front().map(|hook| hook.name()), Some(hook_ro_name));
        let result = poll!(poll_fn(|cx| controller.poll_next_hook(cx, context, false)));
        assert_eq!(result.map(|result| { result.is_err() }), Poll::Ready(true));

        assert!(controller.running_hook_with_db_write.is_some());
        assert_eq!(controller.hooks.len(), hooks_len - 1)
    }
}
