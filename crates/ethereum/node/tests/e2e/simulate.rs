use crate::utils::eth_payload_attributes;
use alloy_primitives::{Address, U256};
use alloy_provider::{network::EthereumWallet, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{
    simulate::{SimBlock, SimulatePayload, SimulatedBlock},
    state::StateOverridesBuilder,
    BlockOverrides, TransactionRequest,
};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Tests that `eth_simulateV1` handles a transaction with `maxFeePerBlobGas` set but no
/// `blob_versioned_hashes` or sidecar. The transaction should be treated as EIP-1559, not
/// EIP-4844.
///
/// Reproduces <https://github.com/paradigmxyz/reth/issues/21809>
#[tokio::test]
async fn test_simulate_v1_with_max_fee_per_blob_gas_only() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec.clone(),
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    let _ = provider.send_transaction(TransactionRequest::default().to(Address::ZERO)).await?;
    node.advance_block().await?;

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .gas_limit(0x52080)
        .max_fee_per_gas(0)
        .max_priority_fee_per_gas(0)
        .max_fee_per_blob_gas(0)
        .value(U256::ZERO)
        .nonce(0)
        .input(Default::default());

    let state_overrides =
        StateOverridesBuilder::default().with_balance(from, U256::from(0x334500u64)).build();

    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides { base_fee: Some(U256::ZERO), ..Default::default() })
        .with_state_overrides(state_overrides)
        .call(tx);

    let payload = SimulatePayload::default().with_validation().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1, "expected exactly 1 simulated block");
    assert_eq!(result[0].calls.len(), 1, "expected exactly 1 call result");
    let call = &result[0].calls[0];
    assert!(call.status, "expected call to succeed");
    assert!(call.error.is_none(), "expected no error");
    assert_eq!(call.max_used_gas, Some(call.gas_used), "expected maxUsedGas in call result");

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_block_gas_limit_override_above_rpc_gascap() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    // 100M — above the default RPC gas cap (50M) and above the 30M genesis block gas limit.
    // The call itself uses ~21K gas, well under both bounds.
    let high_gas_limit = 100_000_000u64;

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .gas_limit(100_000)
        .max_fee_per_gas(0)
        .max_priority_fee_per_gas(0)
        .value(U256::ZERO)
        .nonce(0);

    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides {
            gas_limit: Some(high_gas_limit),
            ..Default::default()
        })
        .call(tx);

    let payload = SimulatePayload::default().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1, "expected exactly 1 simulated block");
    assert_eq!(
        result[0].inner.header.gas_limit, high_gas_limit,
        "simulated block should reflect the overridden gas limit"
    );
    assert_eq!(result[0].calls.len(), 1, "expected exactly 1 call result");
    assert!(
        result[0].calls[0].status,
        "call should succeed, got error: {:?}",
        result[0].calls[0].error
    );

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_explicit_call_gas_clamped_to_rpc_gascap() -> eyre::Result<()> {
    use alloy_consensus::Transaction as _;

    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    // Default --rpc.gascap is 50M.
    let rpc_gascap_default = 50_000_000u64;
    // Block-level override above rpc.gascap — allowed (matches the first fix).
    let block_gas_limit = 100_000_000u64;
    // Per-call explicit gas above rpc.gascap — op-geth clamps this to rpc.gascap.
    let explicit_call_gas = 80_000_000u64;

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .gas_limit(explicit_call_gas)
        .max_fee_per_gas(0)
        .max_priority_fee_per_gas(0)
        .value(U256::ZERO)
        .nonce(0);

    let sim_block = SimBlock::default()
        .with_block_overrides(BlockOverrides {
            gas_limit: Some(block_gas_limit),
            ..Default::default()
        })
        .call(tx);

    let payload = SimulatePayload::default().with_full_transactions().extend(sim_block);

    let result: Vec<SimulatedBlock> =
        provider.raw_request("eth_simulateV1".into(), (&payload, "latest")).await?;

    assert_eq!(result.len(), 1, "expected exactly 1 simulated block");
    assert_eq!(
        result[0].inner.header.gas_limit, block_gas_limit,
        "simulated block should reflect the overridden block gas limit",
    );
    assert_eq!(result[0].calls.len(), 1, "expected exactly 1 call result");
    assert!(
        result[0].calls[0].status,
        "call should succeed after clamp, got error: {:?}",
        result[0].calls[0].error
    );

    let txs = result[0]
        .inner
        .transactions
        .as_transactions()
        .expect("full transactions should be returned");
    assert_eq!(txs.len(), 1, "expected exactly 1 transaction in simulated block");
    assert_eq!(
        txs[0].gas_limit(),
        rpc_gascap_default,
        "explicit call.gas ({explicit_call_gas}) above --rpc.gascap ({rpc_gascap_default}) \
         must be clamped to --rpc.gascap; otherwise the RPC DoS envelope widens"
    );

    Ok(())
}

#[tokio::test]
async fn test_simulate_v1_too_many_blocks_error() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        chain_spec,
        false,
        Default::default(),
        eth_payload_attributes,
    )
    .await?;
    let node = nodes.pop().unwrap();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(wallet.wallet_gen().swap_remove(0)))
        .connect_http(node.rpc_url());

    let payload: SimulatePayload<TransactionRequest> =
        (0..257).fold(SimulatePayload::default(), |payload, _| payload.extend(SimBlock::default()));

    let err = provider
        .raw_request::<_, Vec<SimulatedBlock>>("eth_simulateV1".into(), (&payload, "latest"))
        .await
        .unwrap_err();
    let err = err.as_error_resp().expect("expected JSON-RPC error response");

    assert_eq!(err.code, -38026);
    assert_eq!(err.message, "too many blocks");

    Ok(())
}
