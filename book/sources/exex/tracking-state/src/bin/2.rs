use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};

use alloy_primitives::BlockNumber;
use futures_util::{FutureExt, TryStreamExt};
use reth::{api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives};
use reth_exex::{ExExContext, ExExEvent};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

struct MyExEx<Node: FullNodeComponents> {
    ctx: ExExContext<Node>,
    /// First block that was committed since the start of the ExEx.
    first_block: Option<BlockNumber>,
    /// Total number of transactions committed.
    transactions: u64,
}

impl<Node: FullNodeComponents> MyExEx<Node> {
    fn new(ctx: ExExContext<Node>) -> Self {
        Self { ctx, first_block: None, transactions: 0 }
    }
}

impl<Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>> Future
    for MyExEx<Node>
{
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        while let Some(notification) = ready!(this.ctx.notifications.try_next().poll_unpin(cx))? {
            if let Some(reverted_chain) = notification.reverted_chain() {
                this.transactions = this.transactions.saturating_sub(
                    reverted_chain.blocks_iter().map(|b| b.body().transactions.len() as u64).sum(),
                );
            }

            if let Some(committed_chain) = notification.committed_chain() {
                this.first_block.get_or_insert(committed_chain.first().number);

                this.transactions += committed_chain
                    .blocks_iter()
                    .map(|b| b.body().transactions.len() as u64)
                    .sum::<u64>();

                this.ctx
                    .events
                    .send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
            }

            if let Some(first_block) = this.first_block {
                info!(%first_block, transactions = %this.transactions, "Total number of transactions");
            }
        }

        Poll::Ready(Ok(()))
    }
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", |ctx| async move { Ok(MyExEx::new(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
