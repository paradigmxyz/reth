//! Support for launching execution extensions.

use std::{fmt, fmt::Debug};

use futures::future;
use reth_chain_state::ForkChoiceSubscriptions;
use reth_chainspec::EthChainSpec;
use reth_exex::{
    ExExContext, ExExHandle, ExExManager, ExExManagerHandle, ExExNotificationSource, Wal,
    DEFAULT_EXEX_MANAGER_CAPACITY,
};
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_primitives::Head;
use reth_provider::CanonStateSubscriptions;
use reth_tracing::tracing::{debug, info};
use tracing::Instrument;

use crate::{common::WithConfigs, exex::BoxedLaunchExEx};

/// Can launch execution extensions.
pub struct ExExLauncher<Node: FullNodeComponents> {
    head: Head,
    extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    components: Node,
    config_container: WithConfigs<<Node::Types as NodeTypes>::ChainSpec>,
}

impl<Node: FullNodeComponents + Clone> ExExLauncher<Node> {
    /// Create a new `ExExLauncher` with the given extensions.
    pub const fn new(
        head: Head,
        components: Node,
        extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
        config_container: WithConfigs<<Node::Types as NodeTypes>::ChainSpec>,
    ) -> Self {
        Self { head, extensions, components, config_container }
    }

    /// Launches all execution extensions.
    ///
    /// Spawns all extensions and returns the handle to the exex manager if any extensions are
    /// installed.
    pub async fn launch(self) -> eyre::Result<Option<ExExManagerHandle>> {
        let Self { head, extensions, components, config_container } = self;

        if extensions.is_empty() {
            // nothing to launch
            return Ok(None)
        }

        info!(target: "reth::cli", "Loading ExEx Write-Ahead Log...");
        let exex_wal = Wal::new(
            config_container
                .config
                .datadir
                .clone()
                .resolve_datadir(config_container.config.chain.chain())
                .exex_wal(),
        )?;

        let mut exex_handles = Vec::with_capacity(extensions.len());
        let mut exexes = Vec::with_capacity(extensions.len());

        for (id, exex) in extensions {
            // create a new exex handle
            let (handle, events, notifications) = ExExHandle::new(
                id.clone(),
                head,
                components.provider().clone(),
                components.block_executor().clone(),
                exex_wal.handle(),
            );
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

            let executor = components.task_executor().clone();
            exexes.push(async move {
                debug!(target: "reth::cli", id, "spawning exex");
                let span = reth_tracing::tracing::info_span!("exex", id);

                // init the exex
                let exex = exex.launch(context).instrument(span.clone()).await.unwrap();

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
        let exex_manager = ExExManager::new(
            components.provider().clone(),
            exex_handles,
            DEFAULT_EXEX_MANAGER_CAPACITY,
            exex_wal,
            components.provider().finalized_block_stream(),
        );
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
                        .send_async(ExExNotificationSource::BlockchainTree, notification.into())
                        .await
                        .expect("blockchain tree notification could not be sent to exex manager");
                }
            },
        );

        info!(target: "reth::cli", "ExEx Manager started");

        Ok(Some(exex_manager_handle))
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
