//! Support for launching execution extensions.

use alloy_eips::{eip2124::Head, BlockNumHash};
use futures::future;
use reth_chain_state::{CanonStateNotifications, ForkChoiceSubscriptions};
use reth_chainspec::EthChainSpec;
use reth_exex::{
    ExExContext, ExExHandle, ExExManager, ExExManagerHandle, ExExNotificationSource, Wal,
    DEFAULT_EXEX_MANAGER_CAPACITY, DEFAULT_WAL_BLOCKS_WARNING,
};
use reth_node_api::{FullNodeComponents, NodePrimitives, NodeTypes, PrimitivesTy};
use reth_provider::CanonStateSubscriptions;
use reth_tracing::tracing::{debug, info};
use std::{fmt, fmt::Debug};
use tokio::sync::broadcast::error::RecvError;
use tracing::Instrument;

use crate::{common::WithConfigs, exex::BoxedLaunchExEx};

/// Can launch execution extensions.
pub struct ExExLauncher<Node: FullNodeComponents> {
    head: Head,
    extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    components: Node,
    config_container: WithConfigs<<Node::Types as NodeTypes>::ChainSpec>,
    /// The threshold for the number of blocks in the WAL before emitting a warning.
    wal_blocks_warning: usize,
    /// The max notification buffer capacity for the ExEx manager.
    capacity: usize,
}

impl<Node: FullNodeComponents> ExExLauncher<Node> {
    /// Create a new `ExExLauncher` with the given extensions.
    pub const fn new(
        head: Head,
        components: Node,
        extensions: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
        config_container: WithConfigs<<Node::Types as NodeTypes>::ChainSpec>,
    ) -> Self {
        Self {
            head,
            extensions,
            components,
            config_container,
            wal_blocks_warning: DEFAULT_WAL_BLOCKS_WARNING,
            capacity: DEFAULT_EXEX_MANAGER_CAPACITY,
        }
    }

    /// Sets the threshold for the number of blocks in the WAL before emitting a warning.
    ///
    /// For L2 chains with faster block times, this value should be increased proportionally
    /// to avoid excessive warnings. For example, a chain with 2-second block times might use
    /// a value 6x higher than the default (768 instead of 128).
    pub const fn with_wal_blocks_warning(mut self, threshold: usize) -> Self {
        self.wal_blocks_warning = threshold;
        self
    }

    /// Sets the max notification buffer capacity for the [`ExExManager`].
    pub const fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Launches all execution extensions.
    ///
    /// Spawns all extensions and returns the handle to the exex manager if any extensions are
    /// installed.
    pub async fn launch(
        self,
    ) -> eyre::Result<Option<ExExManagerHandle<PrimitivesTy<Node::Types>>>> {
        let Self { head, extensions, components, config_container, wal_blocks_warning, capacity } =
            self;
        let head = BlockNumHash::new(head.number, head.hash);

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
                components.evm_config().clone(),
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
                let exex = exex.launch(context).instrument(span.clone()).await?;

                // spawn it as a crit task
                executor.spawn_critical_task(
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

                Ok::<(), eyre::Error>(())
            });
        }

        future::try_join_all(exexes).await?;

        // spawn exex manager
        debug!(target: "reth::cli", "spawning exex manager");
        let exex_manager = ExExManager::new(
            components.provider().clone(),
            exex_handles,
            capacity,
            exex_wal,
            components.provider().finalized_block_stream(),
        )
        .with_wal_blocks_warning(wal_blocks_warning);
        let exex_manager_handle = exex_manager.handle();
        components.task_executor().spawn_critical_task("exex manager", async move {
            exex_manager.await.expect("exex manager crashed");
        });

        // send notifications from the blockchain tree to exex manager
        components.task_executor().spawn_critical_task(
            "exex manager blockchain tree notifications",
            forward_canon_state_notifications(
                components.provider().subscribe_to_canonical_state(),
                exex_manager_handle.clone(),
            ),
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
            .field("wal_blocks_warning", &self.wal_blocks_warning)
            .finish()
    }
}

/// Forwards canonical state notifications to the [`ExExManager`] until the subscription closes.
///
/// Waiting for buffer capacity stops reading the subscription, so a slow `ExEx` makes it fall
/// behind. Once it lags, the missed blocks are gone and the manager would never receive another
/// notification, so this panics to fail the critical task instead of stalling every `ExEx`.
async fn forward_canon_state_notifications<N: NodePrimitives>(
    mut notifications: CanonStateNotifications<N>,
    mut handle: ExExManagerHandle<N>,
) {
    loop {
        match notifications.recv().await {
            Ok(notification) => handle
                .send_async(ExExNotificationSource::BlockchainTree, notification.into())
                .await
                .expect("blockchain tree notification could not be sent to exex manager"),
            Err(RecvError::Lagged(skipped)) => panic!(
                "exex manager missed {skipped} blockchain tree notifications while its buffer was full"
            ),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chain_state::CanonStateNotification;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn commit() -> CanonStateNotification {
        CanonStateNotification::Commit { new: Arc::new(Default::default()) }
    }

    #[tokio::test]
    #[should_panic(
        expected = "exex manager missed 1 blockchain tree notifications while its buffer was full"
    )]
    async fn forward_canon_state_notifications_panics_when_lagged() {
        let (tx, rx) = broadcast::channel(1);
        tx.send(commit()).unwrap();
        tx.send(commit()).unwrap();

        forward_canon_state_notifications(rx, ExExManagerHandle::empty()).await;
    }

    #[tokio::test]
    async fn forward_canon_state_notifications_returns_when_closed() {
        let (tx, rx) = broadcast::channel::<CanonStateNotification>(1);
        drop(tx);

        forward_canon_state_notifications(rx, ExExManagerHandle::empty()).await;
    }
}
