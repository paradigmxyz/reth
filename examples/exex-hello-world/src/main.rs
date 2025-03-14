//! Example for a simple Execution Extension
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-exex-hello-world -- node --dev --dev.block-time 5s
//! ```

use futures::TryStreamExt;
use reth_ethereum::{
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{
        api::{FullNodeComponents, NodeTypes},
        EthereumNode,
    },
    EthPrimitives,
};
use reth_tracing::tracing::info;

async fn my_exex<Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>>(
    mut ctx: ExExContext<Node>,
) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.try_next().await? {
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
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(async move |builder, _| {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("my-exex", async move |ctx| Ok(my_exex(ctx)))
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
