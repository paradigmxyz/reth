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
#[ignore = "depends on alloy-rs/alloy#3651"]
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
    assert!(result[0].calls[0].status, "expected call to succeed");
    assert!(result[0].calls[0].error.is_none(), "expected no error");

    Ok(())
}
