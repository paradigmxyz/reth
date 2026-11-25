//! Tests for the notifications an `ExEx` receives from a running node.

use alloy_primitives::B256;
use futures::{future, TryStreamExt};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{node::Finality, wait::WAIT_TIMEOUT, E2ETestSetupExt};
use reth_exex::{ExExHead, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_provider::Chain;
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc::{self, UnboundedReceiver};

/// Receives the next notification of the `ExEx` and returns the hashes of its reverted and
/// committed blocks.
async fn recv_block_hashes(
    notifications_rx: &mut UnboundedReceiver<ExExNotification>,
) -> eyre::Result<(Option<Vec<B256>>, Option<Vec<B256>>)> {
    let notification = tokio::time::timeout(WAIT_TIMEOUT, notifications_rx.recv())
        .await?
        .ok_or_else(|| eyre::eyre!("ExEx stopped before sending a notification"))?;
    let hashes = |chain: Option<Arc<Chain>>| {
        chain.map(|chain| chain.blocks_iter().map(|block| block.hash()).collect())
    };
    Ok((hashes(notification.reverted_chain()), hashes(notification.committed_chain())))
}

/// An `ExEx` receives the reverted and the committed blocks of every reorg of the node.
#[tokio::test]
async fn exex_receives_reorgs() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (notifications_tx, mut notifications_rx) = mpsc::unbounded_channel();
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_node_builder_modifier(move |builder| {
            let notifications_tx = notifications_tx.clone();
            builder.install_exex("reorgs", |mut ctx| async move {
                Ok(async move {
                    while let Some(notification) = ctx.notifications.try_next().await? {
                        notifications_tx.send(notification)?;
                    }
                    Ok(())
                })
            })
        })
        .build_single()
        .await?;
    node.set_finality(Finality::Keep);
    let genesis = node.block_hash(0);

    let a1 = node.advance_block().await?.block().hash();
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (None, Some(vec![a1])));

    let b1 = node.advance_block_on(genesis).await?.block().hash();
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (Some(vec![a1]), Some(vec![b1])));

    node.reorg_to(a1).await?;
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (Some(vec![b1]), Some(vec![a1])));

    Ok(())
}

/// An `ExEx` whose head was reorged out before it processed the reorg gets the head reverted from
/// the WAL after a restart, then the new canonical blocks backfilled before resuming live delivery.
#[tokio::test]
async fn exex_reverts_orphaned_head_from_wal_after_restart() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let exex_head = Arc::new(OnceLock::new());
    let (notifications_tx, mut notifications_rx) = mpsc::unbounded_channel();
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .with_node_builder_modifier(move |builder| {
            let exex_head = exex_head.clone();
            let notifications_tx = notifications_tx.clone();
            builder.install_exex("wal-replay", |mut ctx| async move {
                Ok(async move {
                    // Before the restart, stop processing after the first notification.
                    let Some(&head) = exex_head.get() else {
                        let Some(notification) = ctx.notifications.try_next().await? else {
                            return Ok(())
                        };
                        if let Some(chain) = notification.committed_chain() {
                            exex_head.get_or_init(|| chain.tip().num_hash());
                        }
                        notifications_tx.send(notification)?;
                        return future::pending().await
                    };

                    ctx.set_notifications_with_head(ExExHead::new(head));
                    while let Some(notification) = ctx.notifications.try_next().await? {
                        notifications_tx.send(notification)?;
                    }
                    Ok(())
                })
            })
        })
        .build_single()
        .await?;
    node.set_finality(Finality::Keep);
    let genesis = node.block_hash(0);

    let a1 = node.advance_block().await?.block().hash();
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (None, Some(vec![a1])));

    // The ExEx does not process the reorg, so its head stays at the orphaned block.
    let b1 = node.advance_block_on(genesis).await?.block().hash();

    let mut node = node.restart().await?;
    assert_eq!(node.block_hash(1), b1);
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (Some(vec![a1]), None));
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (None, Some(vec![b1])));

    let b2 = node.advance_block().await?.block().hash();
    assert_eq!(recv_block_hashes(&mut notifications_rx).await?, (None, Some(vec![b2])));

    Ok(())
}
