use std::sync::Arc;

use reth::{rpc::types::engine::PayloadStatusEnum, tasks::TaskManager};
use reth_e2e_test_utils::{transaction::TransactionTestContext, TestNodeGenerator};

use reth_node_ethereum::EthereumNode;
use reth_primitives::{b256, ChainSpecBuilder, Genesis, MAINNET};
use reth_transaction_pool::TransactionPool;

use crate::utils::eth_payload_attributes;

#[tokio::test]
async fn can_restore_blob_tx_on_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let mut node = TestNodeGenerator::<EthereumNode>::new(chain_spec, exec).gen().await?;

    let blob_wallet = node.wallets.pop().unwrap();
    let second_wallet = node.wallets.pop().unwrap();

    // inject normal tx
    let raw_tx = second_wallet.eip1559().await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;

    // build payload with normal tx
    let (payload, attributes) = node.new_payload(eth_payload_attributes).await?;

    // clean the pool
    node.inner.pool.remove_transactions(vec![tx_hash]);

    // build blob tx
    let blob_tx = blob_wallet.eip4844().await;

    // inject blob tx to the pool
    let blob_tx_hash = node.rpc.inject_tx(blob_tx).await?;
    // fetch it from rpc
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // validate sidecar
    let versioned_hashes = TransactionTestContext::validate_sidecar(envelope);

    // build a payload
    let (blob_payload, blob_attr) = node.new_payload(eth_payload_attributes).await?;

    // submit the blob payload
    let blob_block_hash = node
        .engine_api
        .submit_payload(blob_payload, blob_attr, PayloadStatusEnum::Valid, versioned_hashes.clone())
        .await?;

    let genesis_hash = b256!("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3");

    let (_, _) = tokio::join!(
        // send fcu with blob hash
        node.engine_api.update_forkchoice(genesis_hash, blob_block_hash),
        // send fcu with normal hash
        node.engine_api.update_forkchoice(genesis_hash, payload.block().hash())
    );

    // submit normal payload
    node.engine_api.submit_payload(payload, attributes, PayloadStatusEnum::Valid, vec![]).await?;

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // expects the blob tx to be back in the pool
    let envelope = node.rpc.envelope_by_hash(blob_tx_hash).await?;
    // make sure the sidecar is present
    TransactionTestContext::validate_sidecar(envelope);

    Ok(())
}
