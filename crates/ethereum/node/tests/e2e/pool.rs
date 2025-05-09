use crate::utils::eth_payload_attributes;
use alloy_consensus::{EthereumTxEnvelope, Transaction, TxEip4844};
use alloy_eips::{eip1559::ETHEREUM_BLOCK_GAS_LIMIT_30M, Encodable2718};
use alloy_genesis::Genesis;
use alloy_primitives::B256;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    node::NodeTestContext, transaction::TransactionTestContext, wallet::Wallet,
};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_primitives_traits::Recovered;
use reth_provider::CanonStateSubscriptions;
use reth_tasks::TaskManager;
use reth_transaction_pool::{
    blobstore::InMemoryBlobStore, validate::ValidTransaction, BlockInfo, CoinbaseTipOrdering,
    EthPooledTransaction, Pool, PoolTransaction, TransactionOrigin, TransactionPool,
    TransactionPoolExt, TransactionValidationOutcome, TransactionValidator,
};
use std::sync::Arc;

/// A transaction validator that determines all transactions to be valid.
///
/// An actual validator impl like
/// [`TransactionValidationTaskExecutor`](reth_transaction_pool::pool::TransactionValidationTaskExecutor)
/// would require up to date db access.
///
/// CAUTION: This validator is not safe to use since it doesn't actually validate the transaction's
/// properties such as chain id, balance, nonce, etc.
#[derive(Debug, Default)]
#[non_exhaustive]
struct OkValidator;

impl TransactionValidator for OkValidator {
    type Transaction = EthPooledTransaction;

    async fn validate_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        // Always return valid
        let authorities = transaction.authorization_list().map(|auths| {
            auths.iter().flat_map(|auth| auth.recover_authority()).collect::<Vec<_>>()
        });
        TransactionValidationOutcome::Valid {
            balance: *transaction.cost(),
            state_nonce: transaction.nonce(),
            bytecode_hash: None,
            transaction: ValidTransaction::Valid(transaction),
            propagate: false,
            authorities,
        }
    }
}

// Test that the pool's maintenance task can correctly handle `CanonStateNotification::Commit`
// events
#[tokio::test]
async fn maintain_txpool_commit() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let executor = tasks.executor();

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
        .testing_node(executor.clone())
        .node(EthereumNode::default())
        .launch()
        .await?;

    let mut node = NodeTestContext::new(node, eth_payload_attributes).await?;

    let wallet = Wallet::default();

    executor.spawn_critical(
        "txpool maintenance task",
        reth_transaction_pool::maintain::maintain_transaction_pool_future(
            node.inner.provider.clone(),
            txpool.clone(),
            node.inner.provider.clone().canonical_state_stream(),
            executor.clone(),
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
