use eyre::Result;
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_tracing::tracing::info;

/// The ExEx struct, representing the initialization and execution of the ExEx.
pub struct ExEx<Node: FullNodeComponents> {
    _ctx: ExExContext<Node>,
}

impl<Node: FullNodeComponents> ExEx<Node> {
    /// Runs the ExEx, which processes notifications and emits events.
    pub async fn run(mut ctx: ExExContext<Node>) -> Result<()> {
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
}
