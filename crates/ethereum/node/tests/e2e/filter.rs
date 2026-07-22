//! E2e tests for the `eth_getFilterChanges` cursor behavior when a poll fails.
//!
//! See <https://github.com/paradigmxyz/reth/issues/26303>

use alloy_primitives::{hex, Address};
use alloy_provider::{
    network::{EthereumWallet, TransactionBuilder},
    Provider,
};
use alloy_rpc_types_eth::{Filter, FilterChanges, TransactionRequest};
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::wallet::Wallet;
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{
    args::{DevArgs, RpcServerArgs},
    node_config::NodeConfig,
};
use reth_node_ethereum::{node::EthereumAddOns, EthereumNode};
use reth_provider::providers::BlockchainProvider;
use reth_tasks::Runtime;
use std::sync::Arc;

/// Init code for a contract whose runtime code is `PUSH1 0 PUSH1 0 LOG0 STOP`, i.e. every plain
/// call to the deployed contract emits exactly one log with empty data and no topics.
const LOG_EMITTER_INIT_CODE: [u8; 15] = hex!("6560006000a0006000526006601af3");

/// A poll that fails because the range's matching logs exceed `--rpc.max-logs-per-response` must
/// not advance the filter cursor: the next poll returns the same error again instead of silently
/// skipping the range.
#[tokio::test]
async fn filter_changes_max_results_error_keeps_cursor() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let node_config = NodeConfig::test()
        .with_chain(chain_spec())
        .with_dev(DevArgs { dev: true, ..Default::default() })
        .with_rpc(RpcServerArgs {
            rpc_max_logs_per_response: 1.into(),
            ..RpcServerArgs::default().with_unused_ports().with_http()
        });

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(Runtime::test())
        .with_types_and_provider::<EthereumNode, BlockchainProvider<_>>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_debug_capabilities()
        .await?;

    let signer = Wallet::default().wallet_gen().swap_remove(0);
    let provider = node
        .rpc_server_handle()
        .eth_http_provider_with_wallet(EthereumWallet::new(signer))
        .expect("http server started");

    let emitter = deploy_emitter(&provider).await?;
    let filter_id = provider.new_filter(&Filter::new().address(emitter)).await?;

    // a range whose logs stay within the limit polls fine
    call_emitter(&provider, emitter).await?;
    let changes = provider.get_filter_changes_dyn(filter_id).await?;
    assert_eq!(changes.as_logs().expect("logs expected").len(), 1);

    // the successful poll advanced the cursor, no new changes
    let changes = provider.get_filter_changes_dyn(filter_id).await?;
    assert!(matches!(changes, FilterChanges::Empty));

    // two more logs in two separate blocks exceed the limit for the next multi block range poll
    let first_log_block = call_emitter(&provider, emitter).await?;
    let second_log_block = call_emitter(&provider, emitter).await?;
    assert!(second_log_block > first_log_block);

    let err = provider.get_filter_changes_dyn(filter_id).await.unwrap_err();
    assert!(err.to_string().contains("query exceeds max results"), "unexpected error: {err}");

    // the failed poll must not have advanced the cursor: polling again returns the same error
    // instead of an empty response that silently skips the range
    let err = provider.get_filter_changes_dyn(filter_id).await.unwrap_err();
    assert!(err.to_string().contains("query exceeds max results"), "unexpected error: {err}");

    // the logs of the skipped range are still retrievable, a single block range is exempt from
    // the max results limit
    let logs = provider
        .get_logs(
            &Filter::new().address(emitter).from_block(first_log_block).to_block(first_log_block),
        )
        .await?;
    assert_eq!(logs.len(), 1);

    Ok(())
}

/// A poll that fails because the range exceeds `--rpc.max-blocks-per-filter` must not advance the
/// filter cursor: the next poll returns the same error again instead of silently skipping the
/// range.
#[tokio::test]
async fn filter_changes_max_blocks_error_keeps_cursor() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let node_config = NodeConfig::test()
        .with_chain(chain_spec())
        .with_dev(DevArgs { dev: true, ..Default::default() })
        .with_rpc(RpcServerArgs {
            rpc_max_blocks_per_filter: 2.into(),
            ..RpcServerArgs::default().with_unused_ports().with_http()
        });

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(Runtime::test())
        .with_types_and_provider::<EthereumNode, BlockchainProvider<_>>()
        .with_components(EthereumNode::components())
        .with_add_ons(EthereumAddOns::default())
        .launch_with_debug_capabilities()
        .await?;

    let signer = Wallet::default().wallet_gen().swap_remove(0);
    let provider = node
        .rpc_server_handle()
        .eth_http_provider_with_wallet(EthereumWallet::new(signer))
        .expect("http server started");

    let emitter = deploy_emitter(&provider).await?;
    let filter_id = provider.new_filter(&Filter::new().address(emitter)).await?;

    // advance the chain more than max blocks per filter past the installed cursor
    for _ in 0..3 {
        call_emitter(&provider, emitter).await?;
    }

    let err = provider.get_filter_changes_dyn(filter_id).await.unwrap_err();
    assert!(err.to_string().contains("query exceeds max block range"), "unexpected error: {err}");

    // the failed poll must not have advanced the cursor: polling again returns the same error
    // instead of an empty response that silently skips the range
    let err = provider.get_filter_changes_dyn(filter_id).await.unwrap_err();
    assert!(err.to_string().contains("query exceeds max block range"), "unexpected error: {err}");

    Ok(())
}

fn chain_spec() -> Arc<ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    )
}

/// Deploys the log emitter contract and returns its address.
async fn deploy_emitter<P: Provider>(provider: &P) -> eyre::Result<Address> {
    let receipt = provider
        .send_transaction(TransactionRequest::default().with_deploy_code(LOG_EMITTER_INIT_CODE))
        .await?
        .get_receipt()
        .await?;
    assert!(receipt.status());
    Ok(receipt.contract_address.expect("deployment creates a contract"))
}

/// Calls the emitter, asserts the call emitted exactly one log and returns the block number the
/// call was mined in. With dev instamine every call lands in its own block.
async fn call_emitter<P: Provider>(provider: &P, emitter: Address) -> eyre::Result<u64> {
    let receipt = provider
        .send_transaction(TransactionRequest::default().with_to(emitter))
        .await?
        .get_receipt()
        .await?;
    assert!(receipt.status());
    assert_eq!(receipt.logs().len(), 1);
    Ok(receipt.block_number.expect("mined receipt has a block number"))
}
