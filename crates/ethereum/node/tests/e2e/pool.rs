use crate::utils::eth_payload_attributes;
use alloy_consensus::{EthereumTxEnvelope, TxEip4844};
use alloy_eips::{eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M, Encodable2718};
use alloy_genesis::Genesis;
use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use futures::StreamExt;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, setup_engine, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_api::{PayloadKind, TreeConfig};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::Recovered;
use reth_provider::CanonStateSubscriptions;
use reth_tasks::Runtime;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, test_utils::OkValidator, BlockInfo, CoinbaseTipOrdering,
    EthPooledTransaction, Pool, PoolTransaction, TransactionOrigin, TransactionPool,
    TransactionPoolExt,
};
use std::{sync::Arc, time::Duration};

// Test that stale transactions could be correctly evicted.
#[tokio::test]
async fn maintain_txpool_stale_eviction() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallet = Wallet::default();

    let config = reth_transaction_pool::maintain::MaintainPoolConfig {
        max_tx_lifetime: Duration::from_secs(1),
        ..Default::default()
    };

    runtime.spawn_critical_task(
        "txpool maintenance task",
        reth_transaction_pool::maintain::maintain_transaction_pool_future(
            node.inner.provider.clone(),
            txpool.clone(),
            node.inner.provider.clone().canonical_state_stream(),
            runtime.clone(),
            config,
        ),
    );

    // create a tx with insufficient gas fee and it will be parked
    let envelop =
        TransactionTestContext::transfer_tx_with_gas_fee(1, Some(8_u128), wallet.inner).await;
    let tx = Recovered::new_unchecked(
        EthereumTxEnvelope::<TxEip4844>::from(envelop.clone()),
        Default::default(),
    );
    let pooled_tx = EthPooledTransaction::new(tx.clone(), 200);

    txpool.add_transaction(TransactionOrigin::External, pooled_tx).await.unwrap();
    assert_eq!(txpool.len(), 1);

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // stale tx should be evicted
    assert_eq!(txpool.len(), 0);

    Ok(())
}

// Test that the pool's maintenance task can correctly handle `CanonStateNotification::Reorg` events
#[tokio::test]
async fn maintain_txpool_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );
    let genesis_hash = chain_spec.genesis_hash();
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallets = Wallet::new(2).wallet_gen();
    let w1 = wallets.first().unwrap();
    let w2 = wallets.last().unwrap();

    runtime.spawn_critical_task(
        "txpool maintenance task",
        reth_transaction_pool::maintain::maintain_transaction_pool_future(
            node.inner.provider.clone(),
            txpool.clone(),
            node.inner.provider.clone().canonical_state_stream(),
            runtime.clone(),
            reth_transaction_pool::maintain::MaintainPoolConfig::default(),
        ),
    );

    // build tx1 from wallet1
    let envelop1 = TransactionTestContext::transfer_tx(1, w1.clone()).await;
    let tx1 = Recovered::new_unchecked(
        EthereumTxEnvelope::<TxEip4844>::from(envelop1.clone()),
        w1.address(),
    );
    let pooled_tx1 = EthPooledTransaction::new(tx1.clone(), 200);
    let tx_hash1 = *pooled_tx1.hash();

    // build tx2 from wallet2
    let envelop2 = TransactionTestContext::transfer_tx(1, w2.clone()).await;
    let tx2 = Recovered::new_unchecked(
        EthereumTxEnvelope::<TxEip4844>::from(envelop2.clone()),
        w2.address(),
    );
    let pooled_tx2 = EthPooledTransaction::new(tx2.clone(), 200);
    let tx_hash2 = *pooled_tx2.hash();

    let block_info = BlockInfo {
        block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
        last_seen_block_hash: B256::ZERO,
        last_seen_block_number: 0,
        pending_basefee: 10,
        pending_blob_fee: Some(10),
    };

    txpool.set_block_info(block_info);

    // add two txs to the pool
    txpool.add_transaction(TransactionOrigin::External, pooled_tx1).await.unwrap();
    txpool.add_transaction(TransactionOrigin::External, pooled_tx2).await.unwrap();

    // inject tx1, make the node advance and eventually generate `CanonStateNotification::Commit`
    // event to propagate to the pool
    let _ = node.rpc.inject_tx(envelop1.encoded_2718().into()).await.unwrap();

    // build a payload based on tx1
    let payload1 = node.new_payload().await?;

    // clean up the internal pool of the provider node
    node.inner.pool.remove_transactions(vec![tx_hash1]);

    // inject tx2, make the node reorg and eventually generate `CanonStateNotification::Reorg` event
    // to propagate to the pool
    let _ = node.rpc.inject_tx(envelop2.encoded_2718().into()).await.unwrap();

    // build a payload based on tx2
    let payload2 = node.new_payload().await?;

    // submit payload1
    let block_hash1 = node.submit_payload(payload1).await?;

    node.update_forkchoice(genesis_hash, block_hash1).await?;

    loop {
        // wait for pool to process `CanonStateNotification::Commit` event correctly, and finally
        // tx1 will be removed and tx2 is still in the pool
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if txpool.get(&tx_hash1).is_none() && txpool.get(&tx_hash2).is_some() {
            break;
        }
    }

    // submit payload2
    let block_hash2 = node.submit_payload(payload2).await?;

    node.update_forkchoice(genesis_hash, block_hash2).await?;

    loop {
        // wait for pool to process `CanonStateNotification::Reorg` event properly, and finally tx1
        // will be added back to the pool and tx2 will be removed.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if txpool.get(&tx_hash1).is_some() && txpool.get(&tx_hash2).is_none() {
            break;
        }
    }

    Ok(())
}

// Test that the pool's maintenance task can correctly handle `CanonStateNotification::Commit`
// events
#[tokio::test]
async fn maintain_txpool_commit() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let runtime = Runtime::test();

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );
    let node_config = NodeConfig::test()
        .with_chain(chain_spec)
        .with_unused_ports()
        .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());
    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
        .testing_node(runtime.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallet = Wallet::default();

    runtime.spawn_critical_task(
        "txpool maintenance task",
        reth_transaction_pool::maintain::maintain_transaction_pool_future(
            node.inner.provider.clone(),
            txpool.clone(),
            node.inner.provider.clone().canonical_state_stream(),
            runtime.clone(),
            reth_transaction_pool::maintain::MaintainPoolConfig::default(),
        ),
    );

    let envelop = TransactionTestContext::transfer_tx(1, wallet.inner).await;
    let tx = Recovered::new_unchecked(
        EthereumTxEnvelope::<TxEip4844>::from(envelop.clone()),
        Default::default(),
    );
    let pooled_tx = EthPooledTransaction::new(tx.clone(), 200);

    let block_info = BlockInfo {
        block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
        last_seen_block_hash: B256::ZERO,
        last_seen_block_number: 0,
        pending_basefee: 10,
        pending_blob_fee: Some(10),
    };

    txpool.set_block_info(block_info);

    txpool.add_transaction(TransactionOrigin::External, pooled_tx).await.unwrap();
    assert_eq!(txpool.len(), 1);

    // make the node advance and eventually generate `CanonStateNotification::Commit` event to
    // propagate to the pool
    let _ = node.rpc.inject_tx(envelop.encoded_2718().into()).await.unwrap();
    let _ = node.advance_block().await.unwrap();

    loop {
        // wait for pool to process `CanonStateNotification::Commit` event correctly, and finally
        // the pool will be cleared
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if txpool.is_empty() {
            break;
        }
    }

    Ok(())
}

// Ensures that transactions reorged out by a canonical ancestor forkchoice update are reinserted
// into the pool and eventually included in the payload built on top of the ancestor.
#[tokio::test]
async fn test_fcu_unwind_reinjects_reorged_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        TreeConfig::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();

    // finalized base block: the unwind is only permitted above the latest known finalized block
    let finalized = node.advance_block().await?;
    let finalized_hash = finalized.block().hash();
    let _ = node.canonical_stream.next().await;

    // the unwind target, committed without advancing finality
    let ancestor = node.build_and_submit_payload().await?;
    let ancestor_hash = ancestor.block().hash();
    node.update_forkchoice(finalized_hash, ancestor_hash).await?;
    let _ = node.canonical_stream.next().await;

    // include a transfer in the next block
    let raw_tx =
        TransactionTestContext::transfer_tx_bytes(chain_spec.chain.id(), wallet.inner).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    let reorged = node.build_and_submit_payload().await?;
    assert!(reorged.block().body().transactions().any(|tx| *tx.hash() == tx_hash));
    node.update_forkchoice(finalized_hash, reorged.block().hash()).await?;
    let _ = node.canonical_stream.next().await;

    // wait until pool maintenance has discarded the now mined transaction
    tokio::time::timeout(Duration::from_secs(10), async {
        while node.inner.pool.get(&tx_hash).is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;

    // An FCU back to the canonical ancestor with payload attributes optimistically unwinds the
    // canonical chain and starts a payload build on top of the ancestor.
    let attrs = node.payload.next_attributes();
    let outcome = node
        .inner
        .add_ons_handle
        .beacon_engine_handle
        .fork_choice_updated(
            ForkchoiceState {
                head_block_hash: ancestor_hash,
                safe_block_hash: B256::ZERO,
                finalized_block_hash: B256::ZERO,
            },
            Some(attrs),
        )
        .await?;
    assert!(outcome.payload_status.is_valid());
    let payload_id = outcome.payload_id.unwrap();

    // the unwind is a reorg of the canonical chain and must revert the reorged block
    let notification = node.canonical_stream.next().await.unwrap();
    assert!(notification
        .reverted()
        .is_some_and(|reverted| reverted.blocks().contains_key(&reorged.block().number)));

    // the reorged transaction must re-enter the pool
    tokio::time::timeout(Duration::from_secs(10), async {
        while node.inner.pool.get(&tx_hash).is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;

    // and must eventually be picked up by the payload job building on the ancestor
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(Ok(best)) =
                node.inner.payload_builder_handle.best_payload(payload_id).await &&
                best.block().body().transactions().any(|tx| *tx.hash() == tx_hash)
            {
                break
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;

    // resolving the payload returns the payload built on the ancestor including the reorged
    // transaction again
    let payload = node
        .inner
        .payload_builder_handle
        .resolve_kind(payload_id, PayloadKind::WaitForPending)
        .await
        .unwrap()?;
    assert_eq!(payload.block().parent_hash, ancestor_hash);
    assert!(payload.block().body().transactions().any(|tx| *tx.hash() == tx_hash));

    Ok(())
}
