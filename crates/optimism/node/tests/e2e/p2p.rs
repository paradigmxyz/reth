use crate::utils::{advance_chain, setup};
use reth::blockchain_tree::error::BlockchainTreeError;
use reth_rpc_types::engine::PayloadStatusEnum;
use std::sync::Arc;

use crate::utils::{advance_chain, setup};
use reth::primitives::BASE_MAINNET;
use reth_e2e_test_utils::wallet::WalletGenerator;
use reth_interfaces::blockchain_tree::error::BlockchainTreeError;
use reth_node_optimism::OptimismNode;
use reth_primitives::{ChainId, ChainSpecBuilder, Genesis};
use reth_rpc_types::engine::PayloadStatusEnum;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_id: ChainId = BASE_MAINNET.chain.into();

    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let genesis: Genesis = serde_json::from_str(include_str!("../assets/genesis.json"))?;
    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(BASE_MAINNET.chain)
            .genesis(genesis)
            .ecotone_activated()
            .build(),
    );

    let mut nodes = TestNetworkBuilder::<OptimismNode>::new(3, chain_spec, exec).build().await?;

    let first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();
    let third_node = nodes.pop().unwrap();

    let mut first_node = first_node.with_wallets(chain_id, 1);
    let wallet = first_node.wallets.pop().unwrap();
    let wallet = Arc::new(Mutex::new(wallet));

    let tip: usize = 90;
    let tip_index: usize = tip - 1;
    let reorg_depth = 2;

    // On first node, create a chain up to block number 300a
    let canonical_payload_chain = first_node
        .advance_many(
            tip as u64,
            |nonce: u64| {
                let wallet = wallet.clone();
                Box::pin(async move {
                    let wallet = wallet.lock().await;
                    wallet.optimism_block_info(nonce).await
                })
            },
            optimism_payload_attributes,
        )
        .await
        .unwrap();
    let canonical_chain =
        canonical_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // On second node, sync optimistically up to block number 88a
    second_node
        .engine_api
        .update_optimistic_forkchoice(canonical_chain[tip_index - reorg_depth])
        .await?;
    second_node
        .wait_until_block_is_available(
            (tip - reorg_depth) as u64,
            canonical_chain[tip_index - reorg_depth],
        )
        .await?;

    // On third node, sync optimistically up to block number 90a
    third_node.engine_api.update_optimistic_forkchoice(canonical_chain[tip_index]).await?;
    third_node.wait_until_block_is_available(tip as u64, canonical_chain[tip_index]).await?;

    //  On second node, create a side chain: 88a -> 89b -> 90b
    wallet.lock().await.nonce -= reorg_depth as u64;
    second_node.payload.timestamp = first_node.payload.timestamp - reorg_depth as u64; // TODO: probably want to make it node agnostic

    let side_payload_chain = second_node
        .advance_many(
            reorg_depth as u64,
            |nonce: u64| {
                let wallet = wallet.clone();
                Box::pin(async move {
                    let wallet = wallet.lock().await;
                    wallet.optimism_block_info(nonce).await
                })
            },
            optimism_payload_attributes,
        )
        .await
        .unwrap();
    let side_chain = side_payload_chain.iter().map(|p| p.0.block().hash()).collect::<Vec<_>>();

    // Creates fork chain by submitting 89b payload.
    // By returning Valid here, op-node will finally return a finalized hash
    let _ = third_node
        .engine_api
        .submit_payload(
            side_payload_chain[0].0.clone(),
            side_payload_chain[0].1.clone(),
            PayloadStatusEnum::Valid,
            Default::default(),
        )
        .await;

    // It will issue a pipeline reorg to 88a, and then make 89b canonical AND finalized.
    third_node.engine_api.update_forkchoice(side_chain[0], side_chain[0]).await?;

    // Make sure we have the updated block
    third_node.wait_unwind((tip - reorg_depth) as u64).await?;
    third_node
        .wait_until_block_is_available(
            side_payload_chain[0].0.block().number,
            side_payload_chain[0].0.block().hash(),
        )
        .await?;

    // Make sure that trying to submit 89a again will result in an invalid payload status, since 89b
    // has been set as finalized.
    let _ = third_node
        .engine_api
        .submit_payload(
            canonical_payload_chain[tip_index - reorg_depth + 1].0.clone(),
            canonical_payload_chain[tip_index - reorg_depth + 1].1.clone(),
            PayloadStatusEnum::Invalid {
                validation_error: BlockchainTreeError::PendingBlockIsFinalized {
                    last_finalized: (tip - reorg_depth) as u64 + 1,
                }
                .to_string(),
            },
            Default::default(),
        )
        .await;

    Ok(())
}
