use crate::utils::eth_payload_attributes;
use alloy_eips::BlockId;
use alloy_genesis::Genesis;
use alloy_primitives::{address, Address, Bytes, ChainId, U256};
use alloy_provider::{ext::DebugApi, ProviderBuilder};
use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionInput, TransactionRequest};
use alloy_rpc_types_trace::geth::{
    AccountState, GethDebugTracingCallOptions, GethDebugTracingOptions, PreStateConfig,
    PreStateFrame,
};
use eyre::Result;
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup;
use reth_node_ethereum::EthereumNode;
use reth_testing_utils::prestate::{extend_genesis_with_prestate, parse_geth_prestate};
use serde::Deserialize;
use std::{collections::BTreeMap, str::FromStr, sync::Arc};

const PRESTATE_SNAPSHOT: &str =
    include_str!("../../../../../testing/prestate/tx-selfdestruct-prestate.json");

#[tokio::test]
async fn debug_trace_call_matches_geth_prestate_snapshot() -> Result<()> {
    reth_tracing::init_test_tracing();

    let mut genesis: Genesis =
        serde_json::from_str(include_str!("../assets/genesis.json")).expect("valid genesis");
    let alloc = parse_geth_prestate(PRESTATE_SNAPSHOT).expect("prestate snapshot");
    extend_genesis_with_prestate(&mut genesis, alloc);

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .build(),
    );

    let (mut nodes, _tasks, _) =
        setup::<EthereumNode>(1, chain_spec, false, eth_payload_attributes).await?;
    let node = nodes.pop().expect("node available");
    let provider = ProviderBuilder::new().connect_http(node.rpc_url());

    let trace: PreStateFrame = provider
        .debug_trace_call_prestate(
            build_selfdestruct_request(),
            BlockId::Number(BlockNumberOrTag::Latest),
            GethDebugTracingCallOptions::new(GethDebugTracingOptions::prestate_tracer(
                PreStateConfig::default(),
            )),
        )
        .await?;

    let expected_frame = expected_snapshot_frame()?;
    let actual_frame = trace;

    let expected_accounts = filtered_accounts(expected_frame);
    let actual_accounts = filtered_accounts(actual_frame);

    assert_eq!(actual_accounts, expected_accounts, "prestate tracer should match geth output");

    Ok(())
}

fn build_selfdestruct_request() -> TransactionRequest {
    let from = address!("0xa7fb5ca286fc3fd67525629048a4de3ba24cba2e");
    let to = address!("0xc77ad0a71008d7094a62cfbd250a2eb2afdf2776");
    let gas_limit = u64::from_str_radix("249f0", 16).expect("gas hex");
    let max_fee_per_gas = u128::from_str_radix("c6432e2d7", 16).expect("max fee");
    let max_priority_fee_per_gas = u128::from_str_radix("68889c2b", 16).expect("tip");
    let nonce = u64::from_str_radix("39af8", 16).expect("nonce hex");
    let input = Bytes::from_str("0xf3fef3a3000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec700000000000000000000000000000000000000000000000000000000000f6b64")
        .expect("tx data");

    let mut request = TransactionRequest::default()
        .from(from)
        .to(to)
        .gas_limit(gas_limit)
        .max_fee_per_gas(max_fee_per_gas)
        .max_priority_fee_per_gas(max_priority_fee_per_gas)
        .nonce(nonce)
        .transaction_type(2)
        .value(U256::ZERO)
        .input(TransactionInput::new(input));
    request.chain_id = Some(ChainId::from(1u64));
    request
}

fn expected_snapshot_frame() -> Result<PreStateFrame> {
    #[derive(Deserialize)]
    struct Snapshot {
        result: serde_json::Value,
    }

    let snapshot: Snapshot = serde_json::from_str(PRESTATE_SNAPSHOT)?;
    Ok(serde_json::from_value(snapshot.result)?)
}

fn filtered_accounts(frame: PreStateFrame) -> BTreeMap<Address, AccountState> {
    let addresses = [
        address!("0xa7fb5ca286fc3fd67525629048a4de3ba24cba2e"),
        address!("0xc77ad0a71008d7094a62cfbd250a2eb2afdf2776"),
        address!("0xdac17f958d2ee523a2206206994597c13d831ec7"),
    ];

    match frame {
        PreStateFrame::Default(mode) => {
            mode.0.into_iter().filter(|(addr, _)| addresses.contains(addr)).collect()
        }
        PreStateFrame::Diff(diff) => diff
            .pre
            .into_iter()
            .chain(diff.post)
            .filter(|(addr, _)| addresses.contains(addr))
            .collect(),
    }
}
