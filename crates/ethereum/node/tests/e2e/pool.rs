use alloy_consensus::{EthereumTxEnvelope, TxEip4844};
use alloy_eips::{
    eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M,
    eip2930::{AccessList, AccessListItem},
    Encodable2718,
};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::TransactionRequest;
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::{
    test_chain_spec, test_chain_spec_builder, transaction::TransactionTestContext,
    wait::poll_until, wallet::Wallet, E2ETestSetupExt,
};
use reth_network::{Peers, PeersInfo};
use reth_node_core::args::TxPoolArgs;
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::Recovered;
use reth_provider::CanonStateSubscriptions;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, test_utils::OkValidator, BlockInfo, CoinbaseTipOrdering,
    EthPooledTransaction, Pool, PoolTransaction, TransactionOrigin, TransactionPool,
    TransactionPoolExt,
};
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn rpc_enforces_minimum_priority_fee() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    const MINIMUM_PRIORITY_FEE: u128 = 1;

    let (node, wallet) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        .with_node_config_modifier(|config| {
            config.with_txpool(TxPoolArgs {
                minimum_priority_fee: Some(MINIMUM_PRIORITY_FEE),
                ..Default::default()
            })
        })
        .build_single()
        .await?;
    let provider = node.rpc_provider();

    let transaction = |max_priority_fee_per_gas| TransactionRequest {
        nonce: Some(0),
        value: Some(U256::from(100)),
        to: Some(TxKind::Call(Address::ZERO)),
        gas: Some(21_000),
        max_fee_per_gas: Some(20_000_000_000),
        max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
        chain_id: Some(1),
        ..Default::default()
    };

    let below_minimum =
        TransactionTestContext::sign_tx_bytes(wallet.inner.clone(), transaction(0)).await;
    let err = provider.send_raw_transaction(&below_minimum).await.unwrap_err();
    assert!(
        err.to_string().contains("transaction priority fee below minimum required priority fee 1"),
        "{err}"
    );
    assert!(node.inner.pool.is_empty());

    let at_minimum =
        TransactionTestContext::sign_tx_bytes(wallet.inner, transaction(MINIMUM_PRIORITY_FEE))
            .await;
    let pending = provider.send_raw_transaction(&at_minimum).await?;
    assert_eq!(node.inner.pool.len(), 1);
    assert!(node.inner.pool.contains(pending.tx_hash()));

    Ok(())
}

// Test that stale transactions could be correctly evicted.
#[tokio::test]
async fn maintain_txpool_stale_eviction() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let (node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let runtime = node.inner.task_executor.clone();

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

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let chain_spec = test_chain_spec(EthereumHardfork::Cancun);
    let genesis_hash = chain_spec.genesis_hash();
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let runtime = node.inner.task_executor.clone();

    let (w1, w2) = (wallet.signer(0), wallet.signer(1));

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

    // wait for pool to process `CanonStateNotification::Commit` event correctly, and finally tx1
    // will be removed and tx2 is still in the pool.
    poll_until("pool to process the commit", || async {
        Ok((txpool.get(&tx_hash1).is_none() && txpool.get(&tx_hash2).is_some()).then_some(()))
    })
    .await?;

    // submit payload2
    let block_hash2 = node.submit_payload(payload2).await?;

    node.update_forkchoice(genesis_hash, block_hash2).await?;

    // wait for pool to process `CanonStateNotification::Reorg` event properly, and finally tx1
    // will be added back to the pool and tx2 will be removed.
    poll_until("pool to process the reorg", || async {
        Ok((txpool.get(&tx_hash1).is_some() && txpool.get(&tx_hash2).is_none()).then_some(()))
    })
    .await?;

    Ok(())
}

// Test that the pool's maintenance task can correctly handle `CanonStateNotification::Commit`
// events
#[tokio::test]
async fn maintain_txpool_commit() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let txpool = Pool::new(
        OkValidator::default(),
        CoinbaseTipOrdering::default(),
        InMemoryBlobStore::default(),
        Default::default(),
    );

    // Directly generate a node to simulate various traits such as `StateProviderFactory` required
    // by the pool maintenance task
    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;
    let runtime = node.inner.task_executor.clone();

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

    // wait for pool to process `CanonStateNotification::Commit` event correctly, and finally the
    // pool will be cleared.
    poll_until("pool to process the commit", || async { Ok(txpool.is_empty().then_some(())) })
        .await?;

    Ok(())
}

// Test that the pool processed the new block once `advance_block_synced` returns.
#[tokio::test]
async fn advance_block_synced_waits_for_pool() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut node, wallet) =
        EthereumNode::test_setup_for(EthereumHardfork::Cancun).build_single().await?;

    for nonce in 0..3 {
        let raw_tx = TransactionTestContext::transfer_tx_bytes_with_nonce(
            wallet.chain_id,
            wallet.inner.clone(),
            nonce,
        )
        .await;
        let tx_hash = node.rpc.inject_tx(raw_tx).await?;

        let payload = node.advance_block_synced().await?;
        let block = payload.block();
        assert!(block.body().transactions().any(|tx| *tx.hash() == tx_hash));

        // the mined transaction is removed from the pool without waiting any further
        let info = node.inner.pool.block_info();
        assert_eq!(info.last_seen_block_hash, block.hash());
        assert_eq!(info.last_seen_block_number, block.header().number);
        assert!(node.inner.pool.is_empty());
    }

    Ok(())
}

/// Amsterdam activation timestamp used by the fork-boundary pool tests. Genesis is at timestamp 0,
/// so the genesis head runs Osaka rules and the first block built at this timestamp is Amsterdam.
const AMSTERDAM_TIMESTAMP: u64 = 1_000;

/// Gas limit of a value transfer with one access-list address under Osaka: 21,000 + 2,400.
/// Amsterdam prices the same transaction at 25,180 intrinsic gas.
const OSAKA_ACCESS_LIST_TRANSFER_GAS: u64 = 23_400;

/// Returns a signed EIP-1559 transfer with one access-list address whose gas limit is its exact
/// Osaka intrinsic cost, so it is valid before Amsterdam and invalid after.
async fn fork_invalidated_transfer(wallet: &Wallet, signer: u32, nonce: u64) -> Bytes {
    let tx = TransactionRequest { chain_id: Some(1), ..Default::default() }
        .nonce(nonce)
        .to(Address::with_last_byte(0x42))
        .value(U256::from(1))
        .gas_limit(OSAKA_ACCESS_LIST_TRANSFER_GAS)
        .max_fee_per_gas(20_000_000_000)
        .max_priority_fee_per_gas(1_000_000_000)
        .access_list(AccessList(vec![AccessListItem {
            address: Address::with_last_byte(0x99),
            storage_keys: vec![],
        }]));
    TransactionTestContext::sign_tx_bytes(wallet.signer(signer), tx).await
}

#[tokio::test]
async fn pending_tx_intrinsically_invalid_after_amsterdam() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        test_chain_spec_builder().osaka_activated().with_amsterdam_at(AMSTERDAM_TIMESTAMP).build(),
    );
    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;
    let provider = node.rpc_provider();

    // Both are accepted under the Osaka rules of the genesis head.
    let stuck = *provider
        .send_raw_transaction(&fork_invalidated_transfer(&wallet, 0, 0).await)
        .await?
        .tx_hash();
    let descendant = *provider
        .send_raw_transaction(
            &TransactionTestContext::transfer_tx_bytes_with_nonce(1, wallet.signer(0), 1).await,
        )
        .await?
        .tx_hash();
    assert_eq!(node.inner.pool.pending_transactions().len(), 2);

    node.set_next_payload_timestamp(AMSTERDAM_TIMESTAMP)?;
    for _ in 0..3 {
        let payload = node.advance_block_synced().await?;
        assert!(payload.block().body().transactions.is_empty(), "nothing from the sender is mined");
    }

    // The same transaction from a fresh sender is rejected under the current rules...
    let err = provider
        .send_raw_transaction(&fork_invalidated_transfer(&wallet, 1, 0).await)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("intrinsic gas too low"), "{err}");

    // ...so the pool removed the old one at the fork and parked its descendant behind the gap.
    assert!(!node.inner.pool.contains(&stuck), "fork-invalidated transaction is removed");
    let queued = node.inner.pool.queued_transactions();
    assert!(queued.iter().any(|tx| *tx.hash() == descendant), "its descendant is queued");

    // A correctly priced replacement fills the gap and both get mined.
    let replacement = TransactionRequest { chain_id: Some(1), ..Default::default() }
        .nonce(0)
        .to(Address::with_last_byte(0x42))
        .value(U256::from(1))
        .gas_limit(30_000)
        .max_fee_per_gas(20_000_000_000)
        .max_priority_fee_per_gas(1_000_000_000);
    let _ = provider
        .send_raw_transaction(
            &TransactionTestContext::sign_tx_bytes(wallet.signer(0), replacement).await,
        )
        .await?;
    let payload = node.advance_block_synced().await?;
    assert_eq!(payload.block().body().transactions.len(), 2, "replacement and descendant mined");
    assert!(node.inner.pool.is_empty());

    Ok(())
}

#[tokio::test]
async fn peer_not_penalized_for_txs_invalidated_by_amsterdam() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        test_chain_spec_builder().osaka_activated().with_amsterdam_at(AMSTERDAM_TIMESTAMP).build(),
    );
    let (mut nodes, wallet) =
        EthereumNode::test_setup(2, chain_spec).with_connect_nodes(false).build().await?;
    let mut b = nodes.pop().unwrap();
    let mut a = nodes.pop().unwrap();

    // Four bad transactions were enough to get the announcing node banned.
    for nonce in 0..4 {
        let _ = a
            .rpc_provider()
            .send_raw_transaction(&fork_invalidated_transfer(&wallet, 0, nonce).await)
            .await?;
    }

    // Both nodes move to the first Amsterdam block before they meet.
    a.set_next_payload_timestamp(AMSTERDAM_TIMESTAMP)?;
    let payload = a.advance_block_synced().await?;
    b.import_payload(payload.clone()).await?;
    b.wait_for_pool_head(payload.block().hash()).await?;
    assert!(a.inner.pool.is_empty(), "A dropped the fork-invalidated transactions");

    let a_id = a.network.record().id;
    a.connect(&mut b).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(b.inner.network.reputation_by_id(a_id).await?, Some(0));
    assert!(b.inner.network.num_connected_peers() > 0, "A stays connected to B");

    Ok(())
}
