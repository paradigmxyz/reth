use alloy_primitives::B256;
use reth_e2e_test_utils::{
    test_chain_spec_builder, test_genesis, transaction::TransactionTestContext, E2ETestSetupExt,
};
use reth_node_ethereum::EthereumNode;
use reth_provider::{HeaderProvider, StageCheckpointReader};
use reth_stages_types::StageId;
use std::sync::Arc;

/// Tests that a node can initialize and advance with a custom genesis block number.
#[tokio::test]
async fn can_run_eth_node_with_custom_genesis_number() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    // Create genesis with custom block number (e.g., 1000)
    let mut genesis = test_genesis();
    genesis.number = Some(1000);
    genesis.parent_hash = Some(B256::random());
    let chain_spec =
        Arc::new(test_chain_spec_builder().genesis(genesis).cancun_activated().build());

    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;

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

    // Advance the chain (block 1001) and assert the block has been committed
    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let (_, payload) = node.inject_and_advance(raw_tx).await?;
    let block_number = payload.block().number;

    // Verify we're at block 1001 (genesis + 1)
    assert_eq!(block_number, 1001, "Block number should be 1001 after advancing from genesis 1000");

    Ok(())
}

/// Tests that block queries respect custom genesis boundaries.
#[tokio::test]
async fn custom_genesis_block_query_boundaries() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let genesis_number = 5000u64;

    let mut genesis = test_genesis();
    genesis.number = Some(genesis_number);
    genesis.parent_hash = Some(B256::random());
    let chain_spec =
        Arc::new(test_chain_spec_builder().genesis(genesis).cancun_activated().build());

    let (node, _) = EthereumNode::test_setup(1, chain_spec).build_single().await?;

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

/// Tests that payloads are built on top of a genesis that is newer than the default payload
/// timestamp of the test context.
#[tokio::test]
async fn can_advance_on_genesis_newer_than_payload_timestamp() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let genesis_timestamp = 2_000_000_000;
    let mut genesis = test_genesis();
    genesis.timestamp = genesis_timestamp;
    let chain_spec =
        Arc::new(test_chain_spec_builder().genesis(genesis).cancun_activated().build());

    let (mut node, wallet) = EthereumNode::test_setup(1, chain_spec).build_single().await?;

    let raw_tx = TransactionTestContext::transfer_tx_bytes(1, wallet.inner).await;
    let (_, payload) = node.inject_and_advance(raw_tx).await?;
    assert!(payload.block().timestamp > genesis_timestamp);

    Ok(())
}
