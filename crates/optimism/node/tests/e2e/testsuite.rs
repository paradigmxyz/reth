use alloy_primitives::{Address, B256};
use eyre::Result;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use reth_e2e_test_utils::testsuite::{
    actions::AssertMineBlock,
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_optimism_chainspec::{OpChainSpecBuilder, OP_MAINNET};
use reth_optimism_node::{OpEngineTypes, OpNode};
use std::{io::Chain, sync::Arc};

#[tokio::test]
async fn test_testsuite_op_assert_mine_block() -> Result<()> {
    reth_tracing::init_test_tracing();

    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(OP_MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals: None,
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}

#[tokio::test]
async fn test_testsuite_op_assert_mine_block_isthmus() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Set up a timestamp that's in the future to ensure all hardforks are active
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() +
            3600; // Add an hour to ensure it's in the future

    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(Chain::dev())
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .isthmus_activated() // Enable Isthmus at genesis
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let withdrawals = Some(vec![]);

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp,
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals, // Include the withdrawals field (empty vector)
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}

#[tokio::test]
async fn test_testsuite_op_assert_mine_block_isthmus_mainnet() -> Result<()> {
    reth_tracing::init_test_tracing();

    // Set up a timestamp that's in the future to ensure all hardforks are active
    let timestamp =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() +
            3600; // Add an hour to ensure it's in the future

    // Create a mainnet chainspec with Isthmus activated at genesis
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            OpChainSpecBuilder::default()
                .chain(OP_MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
                .isthmus_activated() // Enable Isthmus at genesis
                .build()
                .into(),
        ))
        .with_network(NetworkSetup::single_node());

    let withdrawals = Some(vec![]);

    let test =
        TestBuilder::new().with_setup(setup).with_action(AssertMineBlock::<OpEngineTypes>::new(
            0,
            vec![],
            Some(B256::ZERO),
            OpPayloadAttributes {
                payload_attributes: alloy_rpc_types_engine::PayloadAttributes {
                    timestamp,
                    prev_randao: B256::random(),
                    suggested_fee_recipient: Address::random(),
                    withdrawals, // Include the withdrawals field (empty vector)
                    parent_beacon_block_root: None,
                },
                transactions: None,
                no_tx_pool: None,
                eip_1559_params: None,
                gas_limit: Some(30_000_000),
            },
        ));

    test.run::<OpNode>().await?;

    Ok(())
}
