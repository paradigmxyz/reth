//! Example for a simple Execution Extension
//!
//! Run with
//!
//! ```sh
//! cargo run -p example-exex-hello-world -- node --dev --dev.block-time 5s
//! ```

use clap::Parser;
use futures::TryStreamExt;
use reth_ethereum::{
    exex::{ExExContext, ExExEvent, ExExNotification},
    node::{api::FullNodeComponents, EthereumNode},
    rpc::eth::EthApiFor,
};
use reth_tracing::tracing::info;
use tokio::sync::oneshot;

#[derive(Parser)]
struct ExExArgs {
    #[arg(long)]
    optimism: bool,
}

async fn my_exex<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
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

/// This is an example of how to access the `EthApi` inside an ExEx. It receives the `EthApi` once
/// the node is launched fully.
async fn ethapi_exex<Node>(
    mut ctx: ExExContext<Node>,
    ethapi_rx: oneshot::Receiver<EthApiFor<Node>>,
) -> eyre::Result<()>
where
    Node: FullNodeComponents,
{
    // Wait for the ethapi to be sent from the main function
    let _ethapi = ethapi_rx.await?;
    info!("Received ethapi inside exex");

    while let Some(notification) = ctx.notifications.try_next().await? {
        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    let args = ExExArgs::parse();

    if args.optimism {
        reth_op::cli::Cli::parse_args().run(|builder, _| {
            Box::pin(async move {
                let handle = builder
                    .node(reth_op::node::OpNode::default())
                    .install_exex("my-exex", async move |ctx| Ok(my_exex(ctx)))
                    .launch()
                    .await?;

                handle.wait_for_node_exit().await
            })
        })
    } else {
        reth_ethereum::cli::Cli::parse_args().run(|builder, _| {
            Box::pin(async move {
                let (ethapi_tx, ethapi_rx) = oneshot::channel();
                let handle = builder
                    .node(EthereumNode::default())
                    .install_exex("my-exex", async move |ctx| Ok(my_exex(ctx)))
                    .install_exex("ethapi-exex", async move |ctx| Ok(ethapi_exex(ctx, ethapi_rx)))
                    .launch()
                    .await?;

                // Retrieve the ethapi from the node and send it to the exex
                let ethapi = handle.node.add_ons_handle.eth_api();
                ethapi_tx.send(ethapi.clone()).expect("Failed to send ethapi to ExEx");

                handle.wait_for_node_exit().await
            })
        })
    }
}
