use clap::Parser;
use futures::Future;
use reth_exex::{ExExContext, ExExEvent};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;

mod cli;
mod crypto;
mod generator;
mod parser;

async fn init_stealthy<Node: FullNodeComponents>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>> {
    Ok(stealthy(ctx))
}

/// Checks every committed chain of blocks for stealth addresses belonging to us according to [ERC-5564](https://eips.ethereum.org/EIPS/eip-5564).
///
/// Assumes that `VIEW_KEY` has been set as an environment variable.
async fn stealthy<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    while let Some(notification) = ctx.notifications.recv().await {
        if let Some(committed_chain) = notification.committed_chain() {
            parser::peek(&committed_chain);

            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }
    }

    Ok(())
}

fn main() -> eyre::Result<()> {
    cli::Cli::parse().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Stealthy", init_stealthy)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
