use futures::Future;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

/// The initialization logic of the ExEx is just an async function.
///
/// During initialization you can wait for resources you need to be up for the ExEx to function,
/// like a database connection.
async fn exex_init<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    Ok(exex(ctx))
}

/// An ExEx is just a future, which means you can implement all of it in an async function!
///
/// This ExEx just prints out whenever either a new chain of blocks being added, or a chain of
/// blocks being re-orged. After processing the chain, emits an [ExExEvent::FinishedHeight] event.
async fn exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(committed_chain = ?new.range(), "Received commit");
            }
            ExExNotification::ChainReorged { old, new } => {
                info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
            }
            ExExNotification::ChainReverted { old } => {
                info!(reverted_chain = ?old.range(), "Received revert");
            }
        };

        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Minimal", exex_init)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use std::{future::poll_fn, pin::pin, sync::Arc, task::Poll};

    use futures::FutureExt;
    use reth::{
        blockchain_tree::noop::NoopBlockchainTree,
        builder::{components::Components, NodeAdapter, NodeConfig},
        network::{config::SecretKey, NetworkConfigBuilder, NetworkManager},
        payload::noop::NoopPayloadBuilderService,
        primitives::Head,
        providers::{
            providers::BlockchainProvider, test_utils::create_test_provider_factory,
            BundleStateWithReceipts, Chain,
        },
        tasks::TaskManager,
        transaction_pool::test_utils::testing_pool,
    };
    use reth_db_common::init::init_genesis;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_exex::{ExExContext, ExExEvent, ExExNotification};
    use reth_node_api::FullNodeTypesAdapter;
    use reth_node_ethereum::{EthEngineTypes, EthEvmConfig, EthereumNode};
    use reth_testing_utils::generators::random_block;
    use tokio::sync::mpsc::error::TryRecvError;

    #[tokio::test]
    async fn exex() -> eyre::Result<()> {
        let transaction_pool = testing_pool();
        let evm_config = EthEvmConfig::default();
        let executor = MockExecutorProvider::default();

        let provider_factory = create_test_provider_factory();
        init_genesis(provider_factory.clone())?;
        let provider = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        let network_manager = NetworkManager::new(
            NetworkConfigBuilder::new(SecretKey::new(&mut rand::thread_rng()))
                .build(provider_factory.clone()),
        )
        .await?;
        let network = network_manager.handle().clone();

        let (_, payload_builder) = NoopPayloadBuilderService::<EthEngineTypes>::new();

        let tasks = TaskManager::current();
        let task_executor = tasks.executor();

        let components = NodeAdapter::<FullNodeTypesAdapter<EthereumNode, _, _>, _> {
            components: Components {
                transaction_pool,
                evm_config,
                executor,
                network,
                payload_builder,
            },
            task_executor,
            provider,
        };

        let block = random_block(&mut rand::thread_rng(), 0, None, Some(0), None)
            .seal_with_senders()
            .ok_or(eyre::eyre!("failed to recover senders"))?;

        let head = Head {
            number: block.number,
            hash: block.hash(),
            difficulty: block.difficulty,
            timestamp: block.timestamp,
            total_difficulty: Default::default(),
        };

        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let (notifications_tx, notifications_rx) = tokio::sync::mpsc::channel(1);

        let chain = Chain::from_block(block, BundleStateWithReceipts::default(), None);
        notifications_tx.send(ExExNotification::ChainCommitted { new: Arc::new(chain) }).await?;

        let ctx = ExExContext {
            head,
            config: NodeConfig::test(),
            reth_config: reth_config::Config::default(),
            events: events_tx,
            notifications: notifications_rx,
            components,
        };

        let mut exex = pin!(super::exex_init(ctx).await?);

        assert_eq!(events_rx.try_recv(), Err(TryRecvError::Empty));

        poll_fn(|cx| {
            assert!(exex.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        let event = events_rx.try_recv()?;
        assert_eq!(event, ExExEvent::FinishedHeight(head.number));

        Ok(())
    }
}
