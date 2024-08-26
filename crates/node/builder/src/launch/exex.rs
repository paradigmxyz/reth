//! Support for launching execution extensions.

use std::{fmt, fmt::Debug};

use futures::future;
use reth_exex::{ExExContext, ExExEvent, ExExHandle, ExExHead, ExExManager, ExExManagerHandle};
use reth_node_api::FullNodeComponents;
use reth_primitives::Head;
use reth_provider::{CanonStateSubscriptions, HeaderProvider};
use reth_tracing::tracing::{debug, info};
use tracing::Instrument;

use crate::{common::WithConfigs, exex::BoxedLaunchExEx};

/// Can launch execution extensions.
pub struct ExExLauncher<Node: FullNodeComponents> {
    head: Head,
    extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    components: Node,
    config_container: WithConfigs,
}

impl<Node: FullNodeComponents + Clone> ExExLauncher<Node> {
    /// Create a new `ExExLauncher` with the given extensions.
    pub const fn new(
        head: Head,
        components: Node,
        extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
        config_container: WithConfigs,
    ) -> Self {
        Self { head, extensions, components, config_container }
    }

    /// Launches all execution extensions.
    ///
    /// Spawns all extensions and returns the handle to the exex manager if any extensions are
    /// installed.
    pub async fn launch(self) -> Option<ExExManagerHandle> {
        let Self { head, extensions, components, config_container } = self;

        if extensions.is_empty() {
            // nothing to launch
            return None
        }

        let mut exex_handles = Vec::with_capacity(extensions.len());
        let mut exexes = Vec::with_capacity(extensions.len());

        for (id, exex) in extensions {
            // create a new exex handle
            let (handle, events, notifications) = ExExHandle::new(id.clone());
            exex_handles.push(handle);

            // create the launch context for the exex
            let context = ExExContext {
                head,
                config: config_container.config.clone(),
                reth_config: config_container.toml_config.clone(),
                components: components.clone(),
                events,
                notifications,
            };
            let events = context.events.clone();

            let executor = components.task_executor().clone();
            let provider = components.provider().clone();
            exexes.push(async move {
                debug!(target: "reth::cli", id, "spawning exex");
                let span = reth_tracing::tracing::info_span!("exex", id);

                // init the exex
                let (exex_head, exex) =
                    exex.launch(context).instrument(span.clone()).await.unwrap();

                // check the exex head against the chain head
                if let Some(exex_head) = exex_head.filter(|exex_head| *exex_head != head.into()) {
                    span.in_scope(|| {
                        let mut revert_to_first_matching = false;
                        let mut backfill = false;
                        if exex_head.number > head.number {
                            // ExEx is ahead of the chain head. Send a `FinishedHeight` event to notify Reth
                            // that the ExEx has processed the blocks, and don't backfill anything.

                            events.send(ExExEvent::FinishedHeight(exex_head.number)).expect("failed to send FinishedHeight event");
                            info!(target: "reth::cli", ?exex_head, "ExEx is ahead of the chain head");
                        } else if exex_head.number < head.number {
                            // ExEx is behind of the chain head. Check if the ExEx head has the same block hash
                            // as the chain head. It can be different in case of a reorg.

                            backfill = true;

                            let exex_head_header = provider.sealed_header(exex_head.number).expect("failed to get sealed header").expect("sealed header not found");
                            if exex_head_header.hash() == exex_head.hash {
                                // If the ExEx head has the same block hash as in the database,
                                // we can backfill the missing blocks.

                                info!(target: "reth::cli", ?exex_head, "ExEx is behind of the chain head");
                            } else {
                                // If the ExEx head has a different block hash than the chain head, we first need
                                // to revert the ExEx head to the first database block that matches by hash,
                                // and then backfill the missing blocks.

                                revert_to_first_matching = true;

                                info!(target: "reth::cli", ?exex_head, "ExEx is behind of the chain head with a non-canonical hash");
                            }
                        } else if exex_head.hash != head.hash {
                            // ExEx is at the chain head, but with a different hash.
                            // We first need to revert the ExEx head to the first database block that matches by hash,
                            // and then backfill the missing blocks.

                            revert_to_first_matching = true;
                            backfill = true;

                            info!(target: "reth::cli", ?exex_head, "ExEx is at the chain head, but with a non-canonical hash");
                        }

                        if revert_to_first_matching {
                            // TODO(alexey): revert to first matching block
                        }

                        if backfill {
                            // TODO(alexey): backfill
                        }
                    });
                }

                // spawn it as a crit task
                executor.spawn_critical(
                    "exex",
                    async move {
                        info!(target: "reth::cli", "ExEx started");
                        match exex.await {
                            Ok(_) => panic!("ExEx {id} finished. ExExes should run indefinitely"),
                            Err(err) => panic!("ExEx {id} crashed: {err}"),
                        }
                    }
                    .instrument(span),
                );
            });
        }

        future::join_all(exexes).await;

        // spawn exex manager
        debug!(target: "reth::cli", "spawning exex manager");
        // todo(onbjerg): rm magic number
        let exex_manager = ExExManager::new(exex_handles, 1024);
        let exex_manager_handle = exex_manager.handle();
        components.task_executor().spawn_critical("exex manager", async move {
            exex_manager.await.expect("exex manager crashed");
        });

        // send notifications from the blockchain tree to exex manager
        let mut canon_state_notifications = components.provider().subscribe_to_canonical_state();
        let mut handle = exex_manager_handle.clone();
        components.task_executor().spawn_critical(
            "exex manager blockchain tree notifications",
            async move {
                while let Ok(notification) = canon_state_notifications.recv().await {
                    handle
                        .send_async(notification.into())
                        .await
                        .expect("blockchain tree notification could not be sent to exex manager");
                }
            },
        );

        info!(target: "reth::cli", "ExEx Manager started");

        Some(exex_manager_handle)
    }
}

impl<Node: FullNodeComponents> Debug for ExExLauncher<Node> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExExLauncher")
            .field("head", &self.head)
            .field("extensions", &self.extensions.iter().map(|(id, _)| id).collect::<Vec<_>>())
            .field("components", &"...")
            .field("config_container", &self.config_container)
            .finish()
    }
}
