//! Tests for `eth_estimateGas` block override support.
//!
//! These cover the Geth-parity fourth `blockOverrides` parameter on
//! `eth_estimateGas`, ensuring it threads through into estimation.

use crate::utils::eth_payload_attributes;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::{network::EthereumWallet, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{state::StateOverride, BlockOverrides, TransactionRequest};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

/// Tests that a `gasLimit` block override bounds the estimation search space:
/// when the block override sets a tiny gas limit, estimating gas for a basic
/// transfer (which needs >= 21000) must fail because it cannot fit.
#[tokio::test]
async fn test_estimate_gas_block_override_gas_limit_bounds_estimation() -> eyre::Result<()> {
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

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    // Basic transfer call which normally requires 21,000 gas.
    let tx = TransactionRequest::default().from(from).to(to).value(U256::ZERO);

    // Sanity check: without overrides, estimation must succeed.
    let baseline: U256 = provider
        .raw_request("eth_estimateGas".into(), (&tx, "latest"))
        .await
        .expect("baseline estimateGas should succeed");
    assert!(baseline >= U256::from(21_000), "expected baseline >= 21000, got {baseline}");

    // With a `gasLimit` block override well below 21,000, estimation must fail.
    let block_overrides = BlockOverrides { gas_limit: Some(1_000), ..Default::default() };
    let state_override = StateOverride::default();

    let err = provider
        .raw_request::<_, U256>(
            "eth_estimateGas".into(),
            (&tx, "latest", &state_override, &block_overrides),
        )
        .await
        .expect_err("estimateGas should fail when gasLimit override < required gas");
    let err = err.as_error_resp().expect("expected JSON-RPC error response");
    assert!(
        err.message.to_lowercase().contains("gas") || err.message.to_lowercase().contains("out of"),
        "expected a gas-related error message, got: {}",
        err.message
    );

    Ok(())
}

/// Tests that a `baseFee` block override is observed by dynamic fee transactions:
/// setting a `baseFee` higher than the request's `maxFeePerGas` must cause
/// estimation to fail with a "fee cap too low" style error.
#[tokio::test]
async fn test_estimate_gas_block_override_base_fee_affects_dynamic_fee() -> eyre::Result<()> {
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

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    // EIP-1559 dynamic fee tx with a fee cap of 1 wei.
    let tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .value(U256::ZERO)
        .max_fee_per_gas(1)
        .max_priority_fee_per_gas(0);

    // With a basefee override above the tx's maxFeePerGas, estimation should fail.
    let block_overrides =
        BlockOverrides { base_fee: Some(U256::from(1_000_000_000u64)), ..Default::default() };
    let state_override = StateOverride::default();

    let err = provider
        .raw_request::<_, U256>(
            "eth_estimateGas".into(),
            (&tx, "latest", &state_override, &block_overrides),
        )
        .await
        .expect_err("estimateGas should fail when baseFee override > maxFeePerGas");
    let err = err.as_error_resp().expect("expected JSON-RPC error response");
    let msg = err.message.to_lowercase();
    assert!(
        msg.contains("fee") || msg.contains("base") || msg.contains("gas price"),
        "expected fee-related error, got: {}",
        err.message
    );

    Ok(())
}

/// Tests that a `blobBaseFee` block override is observed during estimation:
/// when the override sets a `blobBaseFee` greater than the request's
/// `maxFeePerBlobGas`, estimation must fail with a blob fee cap error.
#[tokio::test]
async fn test_estimate_gas_block_override_blob_base_fee_triggers_cap_too_low() -> eyre::Result<()> {
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

    let from: Address = "0xc000000000000000000000000000000000000000".parse()?;
    let to: Address = "0xc100000000000000000000000000000000000000".parse()?;

    // Construct a blob transaction with a low max_fee_per_blob_gas. We don't
    // include sidecars here; the request must still be treated as EIP-4844
    // because of `blob_versioned_hashes` being populated.
    let mut tx = TransactionRequest::default()
        .from(from)
        .to(to)
        .value(U256::ZERO)
        .max_fee_per_gas(1_000_000_000u128)
        .max_priority_fee_per_gas(0)
        .max_fee_per_blob_gas(1);
    tx.blob_versioned_hashes = Some(vec![B256::ZERO]);

    // Override `blobBaseFee` to a value much higher than the tx's max_fee_per_blob_gas.
    let block_overrides =
        BlockOverrides { blob_base_fee: Some(U256::from(1_000_000_000u64)), ..Default::default() };
    let state_override = StateOverride::default();

    let err = provider
        .raw_request::<_, U256>(
            "eth_estimateGas".into(),
            (&tx, "latest", &state_override, &block_overrides),
        )
        .await
        .expect_err("estimateGas should fail when blobBaseFee override > maxFeePerBlobGas");
    let err = err.as_error_resp().expect("expected JSON-RPC error response");
    let msg = err.message.to_lowercase();
    assert!(
        msg.contains("blob") || msg.contains("fee"),
        "expected blob-fee-related error, got: {}",
        err.message
    );

    Ok(())
}
