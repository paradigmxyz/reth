//! E2E test implementations using the e2e test framework for engine tree functionality.

use crate::tree::TreeConfig;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{MakeCanonical, ProduceBlocks},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Test that verifies forkchoice update and canonical chain insertion functionality.
#[tokio::test]
async fn test_engine_tree_fcu_canon_chain_insertion_e2e() -> Result<()> {
    reth_tracing::init_test_tracing();

    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(
                    serde_json::from_str(include_str!(
                        "../../../../e2e-test-utils/src/testsuite/assets/genesis.json"
                    ))
                    .unwrap(),
                )
                .cancun_activated()
                .build(),
        ))
        .with_network(NetworkSetup::single_node())
        .with_tree_config(
            TreeConfig::default().with_legacy_state_root(false).with_has_enough_parallelism(true),
        );

    let test = TestBuilder::new()
        .with_setup(setup)
        // produce one block
        .with_action(ProduceBlocks::<EthEngineTypes>::new(1))
        // make it canonical via forkchoice update
        .with_action(MakeCanonical::new())
        // extend with 3 more blocks
        .with_action(ProduceBlocks::<EthEngineTypes>::new(3))
        // make the latest block canonical
        .with_action(MakeCanonical::new());

    test.run::<EthereumNode>().await?;

    Ok(())
}
