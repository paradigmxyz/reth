use crate::utils::eth_payload_attributes;
use alloy_genesis::Genesis;
use alloy_primitives::B256;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{setup, transaction::TransactionTestContext};
use reth_node_ethereum::EthereumNode;
use reth_provider::{HeaderProvider, StageCheckpointReader};
use reth_stages_types::StageId;
use std::sync::Arc;

/// Tests that a node can initialize and advance with a custom genesis block number.
#[tokio::test]
async fn can_run_eth_node_with_custom_genesis_number() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Create genesis with custom block number (e.g., 1000)
    let mut genesis: Genesis =
        serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    genesis.number = Some(1000);
    genesis.parent_hash = Some(B256::random());

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) =
        setup::<EthereumNode>(1, chain_spec, false, eth_payload_attributes).await?;

    let mut node = nodes.pop().unwrap();

    // Verify stage checkpoints are initialized to genesis block number (1000)
    for stage in StageId::ALL {
        let checkpoint = node.inner.provider.get_stage_checkpoint(stage)?;
        assert!(checkpoint.is_some(), "Stage {:?} checkpoint should exist", stage);
        assert_eq!(
            checkpoint.unwrap().block_number,
            1000,
            "Stage {:?} checkpoint should be at genesis block 1000",
            stage
        );
    }

    // Advance the chain (block 1001)
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let tx_hash = node.rpc.inject_tx(raw_tx).await?;
    let payload = node.advance_block().await?;

    let block_hash = payload.block().hash();
    let block_number = payload.block().number;

    // Verify we're at block 1001 (genesis + 1)
    assert_eq!(block_number, 1001, "Block number should be 1001 after advancing from genesis 1000");

    // Assert the block has been committed
    node.assert_new_block(tx_hash, block_hash, block_number).await?;

    Ok(())
}

/// Tests that block queries respect custom genesis boundaries.
#[tokio::test]
async fn custom_genesis_block_query_boundaries() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let genesis_number = 5000u64;

    let mut genesis: Genesis =
        serde_json::from_str(include_str!("../assets/genesis.json")).unwrap();
    genesis.number = Some(genesis_number);
    genesis.parent_hash = Some(B256::random());

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _wallet) =
        setup::<EthereumNode>(1, chain_spec, false, eth_payload_attributes).await?;

    let node = nodes.pop().unwrap();

    // Query genesis block should succeed
    let genesis_header = node.inner.provider.header_by_number(genesis_number)?;
    assert!(genesis_header.is_some(), "Genesis block at {} should exist", genesis_number);

    // Query blocks before genesis should return None
    for block_num in [0, 1, genesis_number - 1] {
        let header = node.inner.provider.header_by_number(block_num)?;
        assert!(header.is_none(), "Block {} before genesis should not exist", block_num);
    }

    Ok(())
}
