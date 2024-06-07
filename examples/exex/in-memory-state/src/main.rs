#![warn(unused_crate_dependencies)]

use reth::providers::BundleStateWithReceipts;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;
use std::{
    future::Future,
    pin::{pin, Pin},
    task::{Context, Poll},
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
        pin!(self.get_mut().start()).poll(cx)
    }
}

impl<Node: FullNodeComponents> InMemoryStateExEx<Node> {
    async fn start(&mut self) -> eyre::Result<()> {
        while let Some(notification) = self.ctx.notifications.recv().await {
            match &notification {
                ExExNotification::ChainCommitted { new } => {
                    info!(committed_chain = ?new.range(), "Received commit");
                }
                ExExNotification::ChainReorged { old, new } => {
                    // revert to block before the reorg
                    self.state.revert_to(new.first().number - 1);
                    info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
                }
                ExExNotification::ChainReverted { old } => {
                    self.state.revert_to(old.first().number - 1);
                    info!(reverted_chain = ?old.range(), "Received revert");
                }
            };

            if let Some(committed_chain) = notification.committed_chain() {
                // extend the state with the new chain
                self.state.extend(committed_chain.state().clone());
                self.ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
            }
        }

        Ok(())
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
        primitives::Receipts,
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

        let block = random_block(&mut rng, 0, None, Some(1), None)
            .seal_with_senders()
            .ok_or(eyre::eyre!("failed to recover senders"))?;
        let state = BundleStateWithReceipts::new(
            BundleState::default(),
            Receipts::from_block_receipt(vec![random_receipt(&mut rng, &block.body[0], None)]),
            block.number,
        );

        handle
            .send_notification_chain_committed(Chain::new(vec![block], state.clone(), None))
            .await?;
        exex.poll_once().await?;

        assert_eq!(exex.get_mut().state, state);

        Ok(())
    }
}
