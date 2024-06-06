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
    use reth::providers::{BlockReader, BundleStateWithReceipts, Chain};
    use reth_exex::{ExExEvent, ExExNotification};
    use reth_exex_test_utils::test_exex_context;
    use tokio::sync::mpsc::error::TryRecvError;

    #[tokio::test]
    async fn exex() -> eyre::Result<()> {
        let (ctx, genesis, provider_factory, mut events_rx, notifications_tx) =
            test_exex_context().await?;

        let head = ctx.head;

        notifications_tx
            .send(ExExNotification::ChainCommitted {
                new: Arc::new(Chain::from_block(genesis, BundleStateWithReceipts::default(), None)),
            })
            .await?;

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
