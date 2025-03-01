//! Tests for the e2e test framework.

use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{GetTransactionCount, MineBlock, SubmitTransaction},
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use std::sync::Arc;

#[tokio::test]
#[ignore = "empty impls"]
async fn test_testsuite_complete_example() {
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default().chain(MAINNET.chain).cancun_activated().build(),
        ))
        .with_network(NetworkSetup::single_node());

    struct MockNode;

    let test = TestBuilder::new(MockNode)
        .with_setup(setup)
        .with_action(SubmitTransaction {
            node_idx: 0,
            raw_tx: vec![1, 2, 3], // example transaction bytes
        })
        .with_action(MineBlock { node_idx: 0, transactions: vec![] })
        .with_action(GetTransactionCount {
            node_idx: 0,
            address: B256::ZERO,
            block_id: BlockId::Number(BlockNumberOrTag::Latest),
        });

    test.run().await.unwrap();
}
