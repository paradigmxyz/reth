#![warn(unused_crate_dependencies)]

use reth::providers::ExecutionOutcome;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

/// An ExEx that keeps track of the entire state in memory
async fn track_state<Node: FullNodeComponents>(mut ctx: ExExContext<Node>) -> eyre::Result<()> {
    // keeps the entire plain execution outcome of the chain in memory
    let mut execution_outcome = ExecutionOutcome::default();

    while let Some(notification) = ctx.notifications.recv().await {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                info!(committed_chain = ?new.range(), "Received commit");
            }
            ExExNotification::ChainReorged { old, new } => {
                // revert to block before the reorg
                execution_outcome.revert_to(new.first().number - 1);
                info!(from_chain = ?old.range(), to_chain = ?new.range(), "Received reorg");
            }
            ExExNotification::ChainReverted { old } => {
                execution_outcome.revert_to(old.first().number - 1);
                info!(reverted_chain = ?old.range(), "Received revert");
            }
        };

        if let Some(committed_chain) = notification.committed_chain() {
            // extend the execution outcome with the new chain
            execution_outcome.extend(committed_chain.execution_outcome().clone());
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().number))?;
        }
    }
    Ok(())
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("in-memory-state", |ctx| async move { Ok(track_state(ctx)) })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}
