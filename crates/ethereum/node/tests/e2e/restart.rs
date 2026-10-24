//! Tests for stopping a node and starting it again on its datadir.

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, U256};
use alloy_provider::Provider;
use futures::TryStreamExt;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    wait::{poll_until, WAIT_TIMEOUT},
    E2ETestSetupExt,
};
use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync};
use reth_exex::ExExEvent;
use reth_network::PeersInfo;
use reth_node_builder::sync::{BackfillContext, BackfillSyncBuilder};
use reth_node_ethereum::EthereumNode;
use reth_provider::{
    providers::ProviderNodeTypes, BlockIdReader, BlockNumReader, DatabaseProviderFactory,
    HeaderProvider,
};
use reth_tasks::Runtime;
use reth_transaction_pool::TransactionPool;
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, Notify};

/// A restarted node reopens the chain it persisted, reinserts its pending local transactions and
/// builds on its head, with both storage layouts.
#[tokio::test]
async fn restart_keeps_persisted_chain() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    for storage_v2 in [false, true] {
        let (mut node, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
            .with_restartable_nodes()
            .with_storage_v2(storage_v2)
            .with_tree_config_modifier(|config| {
                config.with_persistence_threshold(0).with_memory_block_buffer_target(0)
            })
            .build_single()
            .await?;

        let recipient = Address::random();
        let mut account = wallet.account(0);
        let raw_tx = account.transfer(recipient, U256::from(100)).await;
        let (tx_hash, _) = node.inject_and_advance(raw_tx).await?;
        let head = node.advance_block().await?.block().hash();
        node.wait_for_persisted_block(2).await?;
        let pending_tx_hash =
            node.rpc.inject_tx(account.transfer(recipient, U256::ONE).await).await?;
        let exit_future =
            node.take_exit_future().ok_or_else(|| eyre::eyre!("the node has no exit future"))?;

        let mut node = node.restart().await?;

        // The consensus engine of the stopped node exited cleanly.
        exit_future.await?;
        assert_eq!(node.inner.provider.best_block_number()?, 2);
        assert_eq!(node.block_hash(2), head);
        assert_eq!(node.inner.provider.safe_block_hash()?, Some(head));
        assert_eq!(node.inner.provider.finalized_block_hash()?, Some(head));
        let receipt = node
            .rpc
            .transaction_receipt(tx_hash)
            .await?
            .ok_or_else(|| eyre::eyre!("receipt of {tx_hash} not found after the restart"))?;
        assert_eq!(receipt.block_number, Some(1));
        assert!(receipt.status());
        assert_eq!(node.rpc_provider().get_balance(recipient).await?, U256::from(100));

        node.wait_for_pool(|pool| pool.contains(&pending_tx_hash)).await?;
        let payload = node.advance_block().await?;
        let block = payload.block();
        assert_eq!(block.number(), 3);
        assert_eq!(block.parent_hash(), head);
        assert!(block.body().transactions().any(|tx| *tx.hash() == pending_tx_hash));
    }

    Ok(())
}

/// Stopping a node persists its canonical blocks that were still in memory, and drops the blocks
/// that are not canonical.
#[tokio::test]
async fn stop_persists_canonical_chain_only() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .build_single()
        .await?;

    node.advance_blocks(3).await?;
    let head = node.block_hash(3);
    // A valid block on top of the head that never becomes canonical.
    let side_block = node.new_payload().await?;
    node.submit_payload(side_block.clone()).await?;
    // The engine keeps recent blocks in memory with the default persistence threshold.
    assert_eq!(node.inner.provider.database_provider_ro()?.best_block_number()?, 0);

    let node = node.restart().await?;

    assert_eq!(node.inner.provider.database_provider_ro()?.best_block_number()?, 3);
    assert_eq!(node.inner.provider.best_block_number()?, 3);
    assert_eq!(node.block_hash(3), head);
    assert!(node.inner.provider.header(side_block.block().hash())?.is_none());

    node.import_payload(side_block).await?;
    assert_eq!(node.inner.provider.best_block_number()?, 4);

    Ok(())
}

/// Nodes are not restartable unless the setup opts in.
#[tokio::test]
async fn stop_requires_restartable_node() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague).build_single().await?;

    let err = node.stop().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "the node is not restartable, launch it with \
         `E2ETestSetupBuilder::with_restartable_nodes` to stop it"
    );

    Ok(())
}

/// A provider of the node that is held across `stop` keeps its database open, which `stop`
/// reports instead of closing the database under it.
#[tokio::test]
async fn stop_fails_while_provider_is_held() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .build_single()
        .await?;

    let provider = node.inner.provider.clone();
    // The held provider never releases the database, so a short timeout fails the same way.
    let release_timeout = Duration::from_millis(100);
    let err = node.stop_with_release_timeout(release_timeout).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!(
            "the database of the stopped node is still in use {release_timeout:?} after the node \
             shut down: drop all handles that hold a provider of the node, e.g. a clone of \
             `inner.provider`, before stopping it"
        )
    );
    assert_eq!(provider.best_block_number()?, 0);

    Ok(())
}

/// A database transaction that is held across `stop` keeps the database open without a handle of
/// it, which `stop` reports instead of leaving it to the next launch.
#[tokio::test]
async fn stop_fails_while_transaction_is_open() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .build_single()
        .await?;

    let provider = node.inner.provider.database_provider_ro()?;
    let err = node.stop().await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "the database of the stopped node is still open: drop all database transactions of the \
         node, e.g. providers of `database_provider_ro`, before stopping it"
    );
    assert_eq!(provider.best_block_number()?, 0);

    Ok(())
}

/// A restarted node reconnects to its peer and syncs the blocks it missed while it was stopped.
#[tokio::test]
async fn restarted_node_syncs_from_peer() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_num_nodes(2)
        .with_restartable_nodes()
        .build()
        .await?;
    let node = nodes.pop().expect("two nodes");
    let mut peer = nodes.pop().expect("two nodes");

    let payload = peer.advance_block().await?;
    node.import_payload(payload).await?;
    let stopped = node.stop().await?;

    peer.advance_blocks(3).await?;
    let head = peer.block_hash(4);
    let network = &peer.inner.network;
    poll_until("the peer to notice the disconnect of the stopped node", || async {
        Ok((network.num_connected_peers() == 0).then_some(()))
    })
    .await?;

    let mut node = stopped.start().await?;
    assert_eq!(node.inner.provider.best_block_number()?, 1);
    node.connect(&mut peer).await;
    node.sync_to(head).await?;
    assert_eq!(node.block_hash(4), head);

    Ok(())
}

/// A restarted node is launched with the node factory and node builder modifiers of the setup
/// again, here an `ExEx` that sees the blocks of the restarted node.
#[tokio::test]
async fn restart_applies_node_factory_and_builder_modifiers() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let launches = Arc::new(AtomicUsize::new(0));
    let (committed_tx, mut committed_rx) = mpsc::unbounded_channel();
    let (node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .with_node({
            let launches = launches.clone();
            move |_| {
                launches.fetch_add(1, Ordering::Relaxed);
                EthereumNode::default()
            }
        })
        .with_node_builder_modifier(move |builder| {
            let committed_tx = committed_tx.clone();
            builder.install_exex("committed-chains", |mut ctx| async move {
                Ok(async move {
                    while let Some(notification) = ctx.notifications.try_next().await? {
                        if let Some(chain) = notification.committed_chain() {
                            ctx.events.send(ExExEvent::FinishedHeight(chain.tip().num_hash()))?;
                            let _ = committed_tx.send(chain);
                        }
                    }
                    Ok(())
                })
            })
        })
        .build_single()
        .await?;

    let mut node = node.restart().await?;
    assert_eq!(launches.load(Ordering::Relaxed), 2);

    let hash = node.advance_block().await?.block().hash();
    tokio::time::timeout(WAIT_TIMEOUT, async {
        while let Some(chain) = committed_rx.recv().await {
            if chain.tip().hash() == hash {
                return Ok(())
            }
        }
        Err(eyre::eyre!("ExEx stopped before receiving block {hash}"))
    })
    .await??;

    Ok(())
}

/// A fatal backfill event must terminate the engine thread even if other engine handles remain.
#[tokio::test]
async fn stop_releases_database_after_backfill_task_drops() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let fail = Arc::new(Notify::new());
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_restartable_nodes()
        .with_backfill({
            let fail = fail.clone();
            move |_| DroppedBackfill(Box::pin(fail.clone().notified_owned()))
        })
        .build_single()
        .await?;
    // Keeping a sender alive prevents channel closure from terminating the engine implicitly.
    let engine = node.engine.clone();
    let exit = node.take_exit_future().expect("the node has an exit future");
    fail.notify_one();
    let err = tokio::time::timeout(WAIT_TIMEOUT, exit).await?.unwrap_err();
    assert_eq!(err.to_string(), "Fatal error in consensus engine");

    node.stop().await?;
    drop(engine);
    Ok(())
}

/// Backfill cancellation after the shutdown branch was polled must still exit successfully.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cancelling_backfill_exits_cleanly() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let runtime = Runtime::test();
    let cancel = Arc::new(Notify::new());
    let (mut node, _) = EthereumNode::test_setup_for(EthereumHardfork::Prague)
        .with_runtime(runtime.clone())
        .with_backfill({
            let runtime = runtime.clone();
            let cancel = cancel.clone();
            move |_| {
                let runtime = runtime.clone();
                let cancel = cancel.clone();
                DroppedBackfill(Box::pin(async move {
                    cancel.notified().await;
                    // Fire shutdown during this backfill poll, without returning Pending to the
                    // select. The task manager runs on another worker while this one is blocked.
                    tokio::task::block_in_place(|| {
                        runtime.handle().block_on(async {
                            drop(
                                tokio::time::timeout(
                                    WAIT_TIMEOUT,
                                    runtime.initiate_graceful_shutdown().unwrap(),
                                )
                                .await
                                .unwrap(),
                            );
                        });
                    });
                }))
            }
        })
        .build_single()
        .await?;
    let exit = node.take_exit_future().expect("the node has an exit future");
    cancel.notify_one();
    tokio::time::timeout(WAIT_TIMEOUT, exit).await??;
    assert!(
        tokio::task::spawn_blocking(move || runtime.graceful_shutdown_with_timeout(WAIT_TIMEOUT))
            .await?
    );
    Ok(())
}

// Emits a dropped-task event when the supplied future completes.
struct DroppedBackfill(Pin<Box<dyn Future<Output = ()> + Send>>);

impl<N: ProviderNodeTypes, C> BackfillSyncBuilder<N, C> for DroppedBackfill {
    type Backfill = Self;

    fn build(self, _ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        Ok(self)
    }
}

impl BackfillSync for DroppedBackfill {
    fn on_action(&mut self, _action: BackfillAction) {}

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        self.0.as_mut().poll(cx).map(|()| BackfillEvent::TaskDropped("channel closed".into()))
    }
}
