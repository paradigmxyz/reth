use alloy_primitives::B256;
use eyre::Result;
use reth_e2e_test_utils::testsuite::{
    actions::AssertMineBlock,
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use reth_optimism_chainspec::{OpChainSpecBuilder, OP_MAINNET};
use reth_optimism_node::{OpEngineTypes, OpNode};
use std::sync::Arc;

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

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(AssertMineBlock::<OpEngineTypes>::new(0, vec![], Some(B256::ZERO)));

    test.run::<OpNode>().await?;

    Ok(())
}
