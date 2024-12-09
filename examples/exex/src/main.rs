use futures::{Future, TryStreamExt};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_api::FullNodeComponents;
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::info;

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
    println!("Inside the EXEX");
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
    reth::cli::Cli::parse_args().run(|builder, _| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("Example", exex_init)
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_exex_test_utils::{test_exex_context, PollOnce};
    use reth_exex::{ExExEvent, ExExHandle, ExExManager, ExExNotification, ExExNotificationSource, Wal};
    use reth_node_ethereum::EthExecutorProvider;
    use tokio::sync::watch;
    use reth_chain_state::ForkChoiceStream;
    use std::{pin::pin, sync::Arc, task::{Context, Poll}};

    #[tokio::test]
    async fn test_exex() -> eyre::Result<()> {
        // need exexManager?

        // Initialize a test Execution Extension context with all dependencies
        let (ctx, mut handle) = test_exex_context().await?;
        let head = ctx.head;

        let chain = Chain::from_block(
            handle.genesis.clone(),
            ExecutionOutcome::default(),
            None,
        );

        let wal = Wal::new(handle.wal_directory.path())?;
        let (finalized_headers_tx, rx) = watch::channel(None);
        let finalized_header_stream = ForkChoiceStream::new(rx);

        let (exex_handle, events_tx, mut notifications) = ExExHandle::new(
            "test_exex".to_string(),
            head,
            handle.provider_factory.clone(),
            EthExecutorProvider::mainnet(),
            wal.handle(),
        );

        let mut exex_manager = pin!(ExExManager::new(
            handle.provider_factory.clone(),
            vec![exex_handle],
            2, // buffer size
            wal,
            finalized_header_stream
        ));

        let mut cx = Context::from_waker(futures::task::noop_waker_ref());

        exex_manager
            .handle()
            .send(ExExNotificationSource::Pipeline, ExExNotification::ChainCommitted {
                new: Arc::new(chain.clone()),
            })?;

        assert!(exex_manager.as_mut().poll(&mut cx)?.is_pending());
        // assert_eq!(
        //     notifications.try_poll_next_unpin(&mut cx)?,
        //     Poll::Ready(Some(ExExNotification::ChainCommitted {
        //         new: Arc::new(chain.clone())
        //     }))
        // );

        let mut exex = pin!(super::exex_init(ctx).await?);
        exex.poll_once().await?;

        events_tx.send(ExExEvent::FinishedHeight((head.number, head.hash).into()))?;

        finalized_headers_tx.send(Some(handle.genesis.header.clone()))?;

        // assert_eq!(
        //     exex_manager.wal.iter_notifications()?.next().transpose()?,
        //     None,
        //     "WAL should be empty after finalization"
        // );

        // Verify FinishedHeight event was emitted
        // handle.assert_event_finished_height((head.number, head.hash).into())?;

        // is 
        // println!("wal: {:?}", wal.num_blocks());
        // Send a notification to the Execution Extension that the chain has been committed
        // handle
        //     .send_notification_chain_committed(chain)
        //     .await?;

        

        // finalized_headers_tx.send(Some(handle.genesis.header.clone()))?;

        // let mut cx = Context::from_waker(futures::task::noop_waker_ref());
        // assert!(exex_manager.as_mut().poll(&mut cx).is_pending());

        // Check that the Execution Extension did not emit any events until we polled it
        // handle.assert_events_empty();

        // Poll the Execution Extension once to process incoming notifications
        // exex.poll_once().await?;

        // wal.finalize((head.number, head.hash).into())?;
        

        // println!("wal 2: {:?}", wal.num_blocks());

        // assert_eq!(
        //     wal.iter_notifications()?.next().transpose()?,
        //     None,
        //     "WAL should be empty after finalization"
        // );

        // assert!(ctx.state)

        Ok(())
    }
}