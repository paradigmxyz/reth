#![warn(unused_crate_dependencies)]

use reth::providers::BundleStateWithReceipts;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

/// An ExEx that keeps track of the entire state in memory
struct InMemoryStateExEx<Node: FullNodeComponents> {
    /// The context of the ExEx
    ctx: ExExContext<Node>,
    /// Entire plain state of the chain
    state: BundleStateWithReceipts,
}

impl<Node: FullNodeComponents> InMemoryStateExEx<Node> {
    /// Create a new instance of the ExEx
    fn new(ctx: ExExContext<Node>) -> Self {
        Self { ctx, state: BundleStateWithReceipts::default() }
    }
}

impl<Node: FullNodeComponents + Unpin> Future for InMemoryStateExEx<Node> {
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        while let Some(notification) = ready!(this.ctx.notifications.poll_recv(cx)) {
            match &notification {
                ExExNotification::ChainCommitted { new } => {
                    info!(committed_chain = ?new.range(), "Received commit");
                }
                ExExNotification::ChainReorged { old, new } => {
                    // revert to block before the reorg
                    this.state.revert_to(new.first().number - 1);
                    info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
                }
                ExExNotification::ChainReverted { old } => {
                    this.state.revert_to(old.first().number - 1);
                    info!(reverted_chain = ?old.range(), "Received revert");
                }
            };

            if let Some(committed_chain) = notification.committed_chain() {
                // extend the state with the new chain
                this.state.extend(committed_chain.state().clone());
                this.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Poll::Ready(Ok(()))
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("in-memory-state", |ctx| async move { Ok(InMemoryStateExEx::new(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use reth::{
        providers::{BundleStateWithReceipts, Chain},
        revm::db::BundleState,
    };
    use reth_exex_test_utils::{test_exex_context, PollOnce};
    use reth_testing_utils::generators::{self, random_block, random_receipt};

    #[tokio::test]
    async fn test_exex() -> eyre::Result<()> {
        let mut rng = &mut generators::rng();

        let (ctx, handle) = test_exex_context().await?;
        let mut exex = pin!(super::InMemoryStateExEx::new(ctx));

        let mut expected_state = BundleStateWithReceipts::default();

        // Generate first block and its state
        let block_1 = random_block(&mut rng, 0, None, Some(1), None)
            .seal_with_senders()
            .ok_or(eyre::eyre!("failed to recover senders"))?;
        let block_number_1 = block_1.number;
        let state_1 = BundleStateWithReceipts::new(
            BundleState::default(),
            vec![random_receipt(&mut rng, &block_1.body[0], None)].into(),
            block_1.number,
            vec![],
        );
        // Extend the expected state with the first block
        expected_state.extend(state_1.clone());

        // Send a notification to the Execution Extension that the chain with the first block has
        // been committed
        handle.send_notification_chain_committed(Chain::new(vec![block_1], state_1, None)).await?;
        exex.poll_once().await?;

        // Assert that the state of the first block has been added to the total state
        assert_eq!(exex.as_mut().state, expected_state);

        // Generate second block and its state
        let block_2 = random_block(&mut rng, 1, None, Some(2), None)
            .seal_with_senders()
            .ok_or(eyre::eyre!("failed to recover senders"))?;
        let state_2 = BundleStateWithReceipts::new(
            BundleState::default(),
            vec![random_receipt(&mut rng, &block_2.body[0], None)].into(),
            block_2.number,
            vec![],
        );
        // Extend the expected state with the second block
        expected_state.extend(state_2.clone());

        // Send a notification to the Execution Extension that the chain with the second block has
        // been committed
        let chain_2 = Chain::new(vec![block_2], state_2, None);
        handle.send_notification_chain_committed(chain_2.clone()).await?;
        exex.poll_once().await?;

        // Assert that the state of the second block has been added to the total state
        assert_eq!(exex.as_mut().state, expected_state);

        // Send a notification to the Execution Extension that the chain with the second block has
        // been reverted
        handle.send_notification_chain_reverted(chain_2).await?;
        exex.poll_once().await?;

        // Assert that the state of the second block has been reverted
        expected_state.revert_to(block_number_1);
        assert_eq!(exex.as_mut().state, expected_state);

        Ok(())
    }
}
