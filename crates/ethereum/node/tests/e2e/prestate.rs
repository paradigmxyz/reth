use alloy_eips::BlockId;
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{address, Address, U256};
use alloy_provider::{ext::DebugApi, Provider};
use alloy_rpc_types_eth::{Bundle, StateContext, Transaction, TransactionRequest};
use alloy_rpc_types_trace::geth::{
    AccountState, DiffMode, GethDebugTracingCallOptions, GethDebugTracingOptions, GethTrace,
    PreStateConfig, PreStateFrame,
};
use eyre::{eyre, Result};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_node_builder::{NodeBuilder, NodeHandle};
use reth_node_core::{args::RpcServerArgs, node_config::NodeConfig};
use reth_node_ethereum::EthereumNode;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use serde::Deserialize;
use std::sync::Arc;

const PRESTATE_SNAPSHOT: &str =
    include_str!("../../../../../testing/prestate/tx-selfdestruct-prestate.json");

/// Replays the selfdestruct transaction via `debug_traceCall` and ensures Reth's prestate matches
/// Geth's captured snapshot.
// <https://github.com/paradigmxyz/reth/issues/19703>
#[tokio::test]
async fn debug_trace_call_matches_geth_prestate_snapshot() -> Result<()> {
    reth_tracing::init_test_tracing();

    let mut genesis: Genesis = MAINNET.genesis().clone();
    genesis.coinbase = address!("0x95222290dd7278aa3ddd389cc1e1d165cc4bafe5");

    let runtime = Runtime::test();

    let expected_frame = expected_snapshot_frame()?;
    let prestate_mode = match &expected_frame {
        PreStateFrame::Default(mode) => mode.clone(),
        _ => return Err(eyre!("snapshot must contain default prestate frame")),
    };

    genesis.alloc.extend(
        prestate_mode
            .0
            .clone()
            .into_iter()
            .map(|(addr, state)| (addr, account_state_to_genesis(state))),
    );

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    let node_config = NodeConfig::test().with_chain(chain_spec).with_rpc(
        RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_http_api(RpcModuleSelection::all_modules().into()),
    );

    let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
        .testing_node(runtime)
        .node(EthereumNode::default())
        .launch()
        .await?;

    let provider = node.rpc_server_handle().eth_http_provider().unwrap();

    // <https://etherscan.io/tx/0x391f4b6a382d3bcc3120adc2ea8c62003e604e487d97281129156fd284a1a89d>
    let tx = r#"{
        "type": "0x2",
        "chainId": "0x1",
        "nonce": "0x39af8",
        "gas": "0x249f0",
        "maxFeePerGas": "0xc6432e2d7",
        "maxPriorityFeePerGas": "0x68889c2b",
        "to": "0xc77ad0a71008d7094a62cfbd250a2eb2afdf2776",
        "value": "0x0",
        "accessList": [],
        "input": "0xf3fef3a3000000000000000000000000dac17f958d2ee523a2206206994597c13d831ec700000000000000000000000000000000000000000000000000000000000f6b64",
        "r": "0x40ab901a8262d5e6fe9b6513996cd5df412526580cab7410c13acc9dd9f6ec93",
        "s": "0x6b76354c8cb1c1d6dbebfd555be9053170f02a648c4b36740e3fd7c6e9499572",
        "yParity": "0x1",
        "v": "0x1",
        "hash": "0x391f4b6a382d3bcc3120adc2ea8c62003e604e487d97281129156fd284a1a89d",
        "blockHash": "0xf9b77bcf8c69544304dff34129f3bdc71f00fdf766c1522ed6ac1382726ead82",
        "blockNumber": "0x1294fd2",
        "transactionIndex": "0x3a",
        "from": "0xa7fb5ca286fc3fd67525629048a4de3ba24cba2e",
        "gasPrice": "0x7c5bcc0e0"
    }"#;
    let tx = serde_json::from_str::<Transaction>(tx).unwrap();
    let request = TransactionRequest::from_recovered_transaction(tx.into_recovered());

    let trace: PreStateFrame = provider
        .debug_trace_call_prestate(
            request,
            BlockId::latest(),
            GethDebugTracingOptions::prestate_tracer(PreStateConfig::default()).into(),
        )
        .await?;

    similar_asserts::assert_eq!(trace, expected_frame);

    Ok(())
}

// <https://github.com/paradigmxyz/reth/issues/23475>
#[tokio::test]
async fn trace_call_prestate_diff_mode_has_no_phantom_gas_credit() -> Result<()> {
    reth_tracing::init_test_tracing();

    let caller = address!("0x3213213213213213213213213213213213213213");
    let beneficiary = address!("0xb20cb55edf81bacfd271f5468e7fc56a3ea4510b");
    let NodeHandle { node, node_exit_future: _ } =
        NodeBuilder::new(prestate_node_config(beneficiary))
            .testing_node(Runtime::test())
            .node(EthereumNode::default())
            .launch()
            .await?;
    let provider = node.rpc_server_handle().eth_http_provider().unwrap();
    let opts = prestate_diff_options();
    let request = phantom_gas_credit_call(caller);

    let trace =
        provider.debug_trace_call_prestate(request, BlockId::latest(), opts.clone().into()).await?;
    assert_no_phantom_gas_credit(&trace, caller, beneficiary)?;

    let bundles = vec![Bundle::from(vec![phantom_gas_credit_call(caller)])];
    let state_context =
        StateContext { block_number: Some(BlockId::latest()), ..Default::default() };
    let call_many_opts: GethDebugTracingCallOptions = opts.into();

    let traces: Vec<Vec<GethTrace>> = provider
        .raw_request("debug_traceCallMany".into(), (bundles, state_context, call_many_opts))
        .await?;
    let trace = traces
        .into_iter()
        .next()
        .and_then(|bundle| bundle.into_iter().next())
        .ok_or_else(|| eyre!("debug_traceCallMany must return one trace"))?;
    let frame = trace.try_into_pre_state_frame()?;
    assert_no_phantom_gas_credit(&frame, caller, beneficiary)?;

    Ok(())
}

fn expected_snapshot_frame() -> Result<PreStateFrame> {
    #[derive(Deserialize)]
    struct Snapshot {
        result: serde_json::Value,
    }

    let snapshot: Snapshot = serde_json::from_str(PRESTATE_SNAPSHOT)?;
    Ok(serde_json::from_value(snapshot.result)?)
}

fn account_state_to_genesis(value: AccountState) -> GenesisAccount {
    let balance = value.balance.unwrap_or_default();
    let code = value.code.filter(|code| !code.is_empty());
    let storage = (!value.storage.is_empty()).then_some(value.storage);

    GenesisAccount::default()
        .with_balance(balance)
        .with_nonce(value.nonce)
        .with_code(code)
        .with_storage(storage)
}

fn prestate_node_config(beneficiary: Address) -> NodeConfig<reth_chainspec::ChainSpec> {
    let mut genesis: Genesis = MAINNET.genesis().clone();
    genesis.coinbase = beneficiary;

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(genesis)
            .cancun_activated()
            .prague_activated()
            .build(),
    );

    NodeConfig::test().with_chain(chain_spec).with_rpc(
        RpcServerArgs::default()
            .with_unused_ports()
            .with_http()
            .with_http_api(RpcModuleSelection::all_modules().into()),
    )
}

fn prestate_diff_options() -> GethDebugTracingOptions {
    GethDebugTracingOptions::prestate_tracer(PreStateConfig {
        diff_mode: Some(true),
        ..Default::default()
    })
}

fn phantom_gas_credit_call(caller: Address) -> TransactionRequest {
    TransactionRequest::default()
        .from(caller)
        .to(caller)
        .gas_limit(0xac6be)
        .gas_price(0xe8d4a51000)
        .value(U256::ZERO)
}

fn assert_no_phantom_gas_credit(
    frame: &PreStateFrame,
    caller: Address,
    beneficiary: Address,
) -> Result<()> {
    let diff = frame.as_diff().ok_or_else(|| eyre!("prestate tracer must return a diff frame"))?;
    assert_no_balance_increase(diff, caller, "caller");
    assert_no_balance_increase(diff, beneficiary, "beneficiary");
    Ok(())
}

fn assert_no_balance_increase(diff: &DiffMode, address: Address, label: &str) {
    let pre_balance = diff.pre.get(&address).and_then(|state| state.balance).unwrap_or_default();
    let post_balance =
        diff.post.get(&address).and_then(|state| state.balance).unwrap_or(pre_balance);

    assert!(
        post_balance <= pre_balance,
        "{label} balance unexpectedly increased in prestate diff: pre={pre_balance}, post={post_balance}"
    );
}
