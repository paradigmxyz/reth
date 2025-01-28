use crate::utils::{advance_with_random_transactions, eth_payload_attributes};
use alloy_provider::{Provider, ProviderBuilder};
use rand::{rngs::StdRng, Rng, SeedableRng};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup, setup_engine, transaction::TransactionTestContext};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

#[tokio::test]
async fn can_sync() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let (mut nodes, _tasks, wallet) = setup::<EthereumNode>(
        2,
        Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ),
        false,
        eth_payload_attributes,
    )
    .await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let mut second_node = nodes.pop().unwrap();
    let mut first_node = nodes.pop().unwrap();

    // Make the first node advance
    let tx_hash = first_node.rpc.inject_tx(raw_tx).await?;

    // make the node advance
    let (payload, _) = first_node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // assert the block has been committed to the blockchain
    first_node.assert_new_block(tx_hash, block_hash, block_number).await?;

    // only send forkchoice update to second node
    second_node.engine_api.update_forkchoice(block_hash, block_hash).await?;

    // expect second node advanced via p2p gossip
    second_node.assert_new_block(tx_hash, block_hash, 1).await?;

    Ok(())
}

#[tokio::test]
async fn e2e_test_send_transactions() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::thread_rng().gen();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {:?}", seed);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _tasks, _) =
        setup_engine::<EthereumNode>(2, chain_spec.clone(), false, eth_payload_attributes).await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new().with_recommended_fillers().on_http(node.rpc_url());

    advance_with_random_transactions(&mut node, 100, &mut rng, true).await?;

    let second_node = nodes.pop().unwrap();
    let second_provider =
        ProviderBuilder::new().with_recommended_fillers().on_http(second_node.rpc_url());

    assert_eq!(second_provider.get_block_number().await?, 0);

    let head =
        provider.get_block_by_number(Default::default(), false.into()).await?.unwrap().header.hash;

    second_node.sync_to(head).await?;

    Ok(())
}

#[tokio::test]
async fn test_long_reorg() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let seed: [u8; 32] = rand::thread_rng().gen();
    let mut rng = StdRng::from_seed(seed);
    println!("Seed: {:?}", seed);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let (mut nodes, _tasks, _) =
        setup_engine::<EthereumNode>(2, chain_spec.clone(), false, eth_payload_attributes).await?;

    let mut first_node = nodes.pop().unwrap();
    let mut second_node = nodes.pop().unwrap();

    let first_provider = ProviderBuilder::new().on_http(first_node.rpc_url());

    // Advance first node 100 blocks.
    advance_with_random_transactions(&mut first_node, 100, &mut rng, false).await?;

    // Sync second node to 20th block.
    let head = first_provider.get_block_by_number(20.into(), false.into()).await?.unwrap();
    second_node.sync_to(head.header.hash).await?;

    // Produce a fork chain with blocks 21.60
    second_node.payload.timestamp = head.header.timestamp;
    advance_with_random_transactions(&mut second_node, 40, &mut rng, true).await?;

    // Reorg first node from 100th block to new 60th block.
    first_node.sync_to(second_node.block_hash(60)).await?;

    // Advance second node 20 blocks and ensure that first node is able to follow it.
    advance_with_random_transactions(&mut second_node, 20, &mut rng, true).await?;
    first_node.sync_to(second_node.block_hash(80)).await?;

    // Ensure that it works the other way around too.
    advance_with_random_transactions(&mut first_node, 20, &mut rng, true).await?;
    second_node.sync_to(first_node.block_hash(100)).await?;

    Ok(())
}
