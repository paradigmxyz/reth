//! Tests for the e2e test framework.

use alloy_primitives::{B256, U256};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::testsuite::{
    actions::{AdvanceBlock, BlockTag, GetTransactionCount, SubmitTransaction},
    assertions::{BlockExists, ValueEquals},
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
            result_id: "tx1".to_string(),
        })
        .with_action(AdvanceBlock {
            node_idx: 0,
            transactions: vec![],
            result_id: "block1".to_string(),
        })
        .with_action(GetTransactionCount {
            node_idx: 0,
            address: B256::ZERO,
            block_tag: BlockTag::Latest,
            result_id: "count1".to_string(),
        })
        .with_assertion(BlockExists { block_hash: B256::ZERO })
        .with_assertion(ValueEquals { value_id: "count1".to_string(), expected: U256::from(1) });

    test.run().await.unwrap();
}
