//! Example tests using the test suite framework.

use crate::testsuite::{
    actions::{AssertMineBlock, SubmitTransaction},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use alloy_primitives::B256;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

#[tokio::test]
async fn test_testsuite_submit_transaction_and_advance_block() -> Result<()> {
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(serde_json::from_str(include_str!("./assets/genesis.json")).unwrap())
                .cancun_activated()
                .build(),
        ))
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new()
        .with_setup(setup)
        .with_action(SubmitTransaction { node_idx: 0, raw_tx: vec![] })
        .with_action(AssertMineBlock {
            node_idx: 0,
            transactions: vec![],
            expected_hash: Some(B256::ZERO),
        });

    test.run::<EthereumNode>().await
}
