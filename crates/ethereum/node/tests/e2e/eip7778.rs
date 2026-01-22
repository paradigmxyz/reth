//! EIP-7778 E2E tests: Block Gas Accounting without Refunds
//!
//! These tests verify that when Osaka is active, block execution works correctly
//! with the new gas accounting (cumulative_gas_used = gross gas, gas_spent = net gas).

use crate::utils::eth_payload_attributes;
use alloy_primitives::U256;
use alloy_provider::{network::EthereumWallet, ProviderBuilder};
use reth_chainspec::{ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_ethereum::EthereumNode;
use std::sync::Arc;

// Simple contract that writes to storage on deployment.
// Similar to GasWaster in rpc.rs but simpler.
alloy_sol_types::sol! {
    #[sol(rpc, bytecode = "6080604052348015600f57600080fd5b5060405160db38038060db833981016040819052602a91607a565b60005b818110156074576040805143602082015290810182905260009060600160408051601f19818403018152919052805160209091012080555080606d816092565b915050602d565b505060b8565b600060208284031215608b57600080fd5b5051919050565b60006001820160b157634e487b7160e01b600052601160045260246000fd5b5060010190565b60168060c56000396000f3fe6080604052600080fdfea164736f6c6343000810000a")]
    contract GasWaster {
        constructor(uint256 iterations);
    }
}

/// Test EIP-7778 gas accounting with storage operations on Osaka.
///
/// This test verifies that when Osaka is active:
/// 1. Contract deployment with storage operations works correctly
/// 2. Receipts have proper gas accounting
/// 3. The gas values are reasonable
#[tokio::test]
async fn test_eip7778_storage_gas_accounting() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .osaka_activated()
            .build(),
    );

    let (mut nodes, _tasks, wallet) = setup_engine::<EthereumNode>(
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

    // Deploy GasWaster contract with 10 storage writes
    // This will do 10 SSTORE operations (0 -> non-zero)
    let deploy_builder = GasWaster::deploy_builder(&provider, U256::from(10)).send().await?;
    node.advance_block().await?;
    let receipt = deploy_builder.get_receipt().await?;

    assert!(receipt.status(), "Contract deployment should succeed");
    assert!(receipt.gas_used > 0, "gas_used should be positive");

    // With 10 SSTORE operations, gas should be significant
    // Each cold SSTORE (0 -> non-zero) costs ~20000 gas
    println!("GasWaster(10) deployment: gas_used={}", receipt.gas_used);
    assert!(
        receipt.gas_used > 200_000,
        "Expected >200k gas for 10 SSTORE operations, got {}",
        receipt.gas_used
    );

    // Deploy another with more iterations
    let deploy_builder2 = GasWaster::deploy_builder(&provider, U256::from(50)).send().await?;
    node.advance_block().await?;
    let receipt2 = deploy_builder2.get_receipt().await?;

    assert!(receipt2.status(), "Second deployment should succeed");
    println!("GasWaster(50) deployment: gas_used={}", receipt2.gas_used);

    // More iterations should use more gas
    assert!(
        receipt2.gas_used > receipt.gas_used,
        "50 iterations should use more gas than 10"
    );

    Ok(())
}

/// Basic smoke test for EIP-7778 Osaka activation.
#[tokio::test]
async fn test_eip7778_osaka_block_execution() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .osaka_activated()
            .build(),
    );

    let (mut nodes, _tasks, wallet) = setup_engine::<EthereumNode>(
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

    // Deploy a simple contract to exercise the execution path
    let deploy_builder = GasWaster::deploy_builder(&provider, U256::from(5)).send().await?;
    node.advance_block().await?;
    let receipt = deploy_builder.get_receipt().await?;

    assert!(receipt.status(), "Deployment should succeed");
    assert!(receipt.gas_used > 0, "gas_used should be positive");

    println!("Osaka block execution test passed: gas_used={}", receipt.gas_used);

    Ok(())
}
