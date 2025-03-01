//! Example tests using the test suite framework.

use crate::testsuite::{
    actions::{
        AssertMineBlock, Call, ForkchoiceUpdated, GetTransactionCount, MineBlock, SubmitTransaction,
    },
    setup::{NetworkSetup, Setup},
    TestBuilder,
};
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use std::sync::Arc;

#[tokio::test]
#[ignore = "empty testsuite impls"]
async fn test_testsuite_submit_transaction_and_advance_block() -> Result<()> {
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default().chain(MAINNET.chain).cancun_activated().build(),
        ))
        .with_network(NetworkSetup::single_node());

    let test = TestBuilder::new(())
        .with_setup(setup)
        .with_action(SubmitTransaction { node_idx: 0, raw_tx: vec![] })
        .with_action(AssertMineBlock {
            node_idx: 0,
            transactions: vec![],
            expected_hash: Some(B256::ZERO),
        });

    test.run().await
}

#[tokio::test]
#[ignore = "empty testsuite impls"]
async fn test_testsuite_chain_reorg() -> Result<()> {
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default().chain(MAINNET.chain).cancun_activated().build(),
        ))
        .with_network(NetworkSetup::multi_node(2));

    let test = TestBuilder::new(())
        .with_setup(setup)
        .with_action(MineBlock { node_idx: 0, transactions: vec![] })
        .with_action(ForkchoiceUpdated {
            node_idx: 1,
            state: (
                B256::ZERO, // head block hash
                B256::ZERO, // safe block hash
                B256::ZERO, // finalized block hash
            ),
            attributes: None,
        })
        .with_action(GetTransactionCount {
            node_idx: 1,
            address: B256::ZERO,
            block_id: BlockId::Number(BlockNumberOrTag::Latest),
        });

    test.run().await
}

#[tokio::test]
#[ignore = "empty testsuite impls"]
async fn test_testsuite_complex_scenario() -> Result<()> {
    let setup = Setup::default()
        .with_chain_spec(Arc::new(
            ChainSpecBuilder::default().chain(MAINNET.chain).cancun_activated().build(),
        ))
        .with_network(NetworkSetup::multi_node(3));

    let test = TestBuilder::new(())
        .with_setup(setup)
        .with_action(SubmitTransaction { node_idx: 0, raw_tx: vec![] })
        .with_action(MineBlock { node_idx: 0, transactions: vec![] })
        .with_action(Call {
            node_idx: 1,
            request: Default::default(),
            block_id: BlockId::Number(BlockNumberOrTag::Latest),
        });

    test.run().await
}
