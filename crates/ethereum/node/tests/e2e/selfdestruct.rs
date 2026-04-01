//! E2E tests for SELFDESTRUCT behavior and output state verification.
//!
//! These tests verify that:
//! - Pre-Dencun: SELFDESTRUCT clears storage and code, output state reflects this
//! - Post-Dencun (EIP-6780): SELFDESTRUCT only works in same-tx creation, state persists
//!
//! We disable prewarming to ensure deterministic cache behavior and verify the execution
//! output state contains the expected account status after SELFDESTRUCT.

use crate::utils::{eth_payload_attributes, eth_payload_attributes_shanghai};
use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{bytes, Address, Bytes, TxKind, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use futures::StreamExt;
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::setup_engine;
use reth_node_api::TreeConfig;
use reth_node_ethereum::EthereumNode;
use reth_revm::db::BundleAccount;
use std::sync::Arc;

const MAX_FEE_PER_GAS: u128 = 20_000_000_000;
const MAX_PRIORITY_FEE_PER_GAS: u128 = 1_000_000_000;

fn cancun_spec() -> Arc<ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    )
}

fn shanghai_spec() -> Arc<ChainSpec> {
    Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .shanghai_activated()
            .build(),
    )
}

fn deploy_tx(from: Address, nonce: u64, init_code: Bytes) -> TransactionRequest {
    TransactionRequest::default()
        .with_from(from)
        .with_nonce(nonce)
        .with_gas_limit(500_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS)
        .with_input(init_code)
        .with_kind(TxKind::Create)
}

fn call_tx(from: Address, to: Address, nonce: u64) -> TransactionRequest {
    TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_nonce(nonce)
        .with_gas_limit(100_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS)
}

fn transfer_tx(from: Address, to: Address, nonce: u64, value: U256) -> TransactionRequest {
    TransactionRequest::default()
        .with_from(from)
        .with_to(to)
        .with_nonce(nonce)
        .with_value(value)
        .with_gas_limit(21_000)
        .with_max_fee_per_gas(MAX_FEE_PER_GAS)
        .with_max_priority_fee_per_gas(MAX_PRIORITY_FEE_PER_GAS)
}

/// Creates init code for a contract that selfdestructs during deployment (same tx).
/// This tests the EIP-6780 exception where SELFDESTRUCT in same tx as creation still works.
///
/// The contract:
/// 1. Stores 0x42 at slot 0
/// 2. Immediately selfdestructs to beneficiary (during init, before returning runtime)
fn selfdestruct_in_constructor_init_code() -> Bytes {
    // Init code that selfdestructs during deployment:
    // PUSH1 0x42, PUSH1 0x00, SSTORE  (store 0x42 at slot 0)
    // PUSH20 <beneficiary>, SELFDESTRUCT
    let mut init = Vec::new();
    init.extend_from_slice(&[0x60, 0x42, 0x60, 0x00, 0x55]); // PUSH1 0x42, PUSH1 0x00, SSTORE
    init.extend_from_slice(&[
        0x73, // PUSH20
        0xde, 0xad, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x01, // beneficiary address
    ]);
    init.push(0xff); // SELFDESTRUCT

    Bytes::from(init)
}

/// Creates init code for a simple contract that:
/// 1. Stores 0x42 at slot 0 during deployment
/// 2. On any call: selfdestructs to beneficiary
///
/// This simpler contract avoids complex branching logic.
fn selfdestruct_contract_init_code() -> Bytes {
    // Runtime: just selfdestruct on any call
    // PUSH20 <beneficiary>
    // SELFDESTRUCT
    let runtime = bytes!(
        "73dead000000000000000000000000000000000001" // PUSH20 beneficiary
        "ff"       // SELFDESTRUCT
    );

    let runtime_len = runtime.len(); // 22 bytes

    // Init code: SSTORE(0, 0x42), CODECOPY, RETURN
    // Total init code before runtime = 17 bytes
    let init_len: u8 = 17;

    let mut init = Vec::new();
    init.extend_from_slice(&[0x60, 0x42, 0x60, 0x00, 0x55]); // PUSH1 0x42, PUSH1 0x00, SSTORE
    init.extend_from_slice(&[0x60, runtime_len as u8, 0x60, init_len, 0x60, 0x00, 0x39]); // CODECOPY
    init.extend_from_slice(&[0x60, runtime_len as u8, 0x60, 0x00, 0xf3]); // RETURN
    init.extend_from_slice(&runtime);

    Bytes::from(init)
}

/// Tests SELFDESTRUCT behavior post-Dencun (Cancun+).
///
/// Post-Dencun (EIP-6780):
/// - SELFDESTRUCT only deletes contract if called in same tx as creation
/// - For existing contracts, SELFDESTRUCT only sends balance, code/storage persist
/// - The output state should NOT mark the account as destroyed
///
/// This test verifies:
/// 1. Contract deploys with storage
/// 2. SELFDESTRUCT in later tx does NOT delete code/storage
/// 3. Output state shows account is NOT destroyed
#[tokio::test]
async fn test_selfdestruct_post_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tree_config = TreeConfig::default().without_prewarming(true).without_state_cache(false);
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, cancun_spec(), false, tree_config, eth_payload_attributes)
            .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Deploy contract that stores 0x42 at slot 0 and selfdestructs on any call
    let pending = provider
        .send_transaction(deploy_tx(signer.address(), 0, selfdestruct_contract_init_code()))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment should succeed");

    let contract_address = receipt.contract_address.expect("Should have contract address");

    // Consume the canonical notification for deployment block
    let _ = node.canonical_stream.next().await;

    // Trigger SELFDESTRUCT by calling the contract
    let pending = provider.send_transaction(call_tx(signer.address(), contract_address, 1)).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Selfdestruct tx should succeed");

    // Get the canonical notification for the selfdestruct block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: post-Dencun, account should NOT be destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_none() || !account_state.unwrap().was_destroyed(),
        "Post-Dencun (EIP-6780): Account should NOT be destroyed when SELFDESTRUCT called on existing contract"
    );

    // Verify via RPC that code and storage persist
    let code_after = provider.get_code_at(contract_address).await?;
    assert!(!code_after.is_empty(), "Post-Dencun: Contract code should persist");

    let slot0_after = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_after, U256::from(0x42), "Post-Dencun: Storage should persist");

    // Send another transaction to the contract address in a new block.
    // This tests cache behavior - if cache has stale data, execution would be incorrect.
    // Post-Dencun: calling the contract should trigger SELFDESTRUCT again (but only transfer
    // balance)
    let pending = provider.send_transaction(call_tx(signer.address(), contract_address, 2)).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Second call to contract should succeed");

    // Consume the canonical notification
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state still shows account NOT destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_none() || !account_state.unwrap().was_destroyed(),
        "Post-Dencun: Account should still NOT be destroyed after second SELFDESTRUCT call"
    );

    // Verify code and storage still persist after the second call
    let code_final = provider.get_code_at(contract_address).await?;
    assert!(!code_final.is_empty(), "Post-Dencun: Contract code should still persist");

    let slot0_final = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_final, U256::from(0x42), "Post-Dencun: Storage should still persist");

    Ok(())
}

/// Tests SELFDESTRUCT in same transaction as creation (post-Dencun).
///
/// Post-Dencun (EIP-6780):
/// - SELFDESTRUCT during the same transaction as creation DOES delete the contract
/// - This is the exception to the rule that SELFDESTRUCT no longer deletes contracts
///
/// This test verifies:
/// 1. Contract selfdestructs during its constructor
/// 2. Contract is deleted (same-tx exception applies)
/// 3. No code or storage remains
/// 4. Since account never existed in DB before, bundle has no entry for it
#[tokio::test]
async fn test_selfdestruct_same_tx_post_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tree_config = TreeConfig::default().without_prewarming(true).without_state_cache(false);
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, cancun_spec(), false, tree_config, eth_payload_attributes)
            .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Deploy contract that selfdestructs during its constructor
    let pending = provider
        .send_transaction(deploy_tx(signer.address(), 0, selfdestruct_in_constructor_init_code()))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment with selfdestruct should succeed");

    // Calculate the contract address (CREATE uses sender + nonce)
    let contract_address = signer.address().create(0);

    // Get the canonical notification for the deployment block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: same-tx SELFDESTRUCT should destroy the account
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_none(),
        "Post-Dencun same-tx: Account was created and selfdestructed in the same transaction, no trace in bundle state"
    );

    // Verify via RPC that code and storage are cleared
    let code = provider.get_code_at(contract_address).await?;
    assert!(code.is_empty(), "Post-Dencun same-tx: Contract code should be deleted");

    let slot0 = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0, U256::ZERO, "Post-Dencun same-tx: Storage should be cleared");

    // Send ETH to the destroyed address in a new block to test cache behavior
    let pending = provider
        .send_transaction(transfer_tx(signer.address(), contract_address, 1, U256::from(1000)))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "ETH transfer to destroyed address should succeed");

    // Consume the canonical notification
    let _ = node.canonical_stream.next().await;

    // Verify code is still empty and account received ETH
    let code_final = provider.get_code_at(contract_address).await?;
    assert!(code_final.is_empty(), "Post-Dencun same-tx: Contract code should remain deleted");

    let balance = provider.get_balance(contract_address).await?;
    assert_eq!(balance, U256::from(1000), "Post-Dencun same-tx: Account should have received ETH");

    Ok(())
}

/// Tests SELFDESTRUCT behavior pre-Dencun (Shanghai).
///
/// Pre-Dencun:
/// - SELFDESTRUCT deletes contract code and storage regardless of when contract was created
/// - The output state MUST mark the account as destroyed
///
/// This test verifies:
/// 1. Contract deploys with storage
/// 2. SELFDESTRUCT deletes code and storage
/// 3. Output state shows account IS destroyed
#[tokio::test]
async fn test_selfdestruct_pre_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tree_config = TreeConfig::default().without_prewarming(true).without_state_cache(false);
    let (mut nodes, wallet) = setup_engine::<EthereumNode>(
        1,
        shanghai_spec(),
        false,
        tree_config,
        eth_payload_attributes_shanghai,
    )
    .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Deploy contract that stores 0x42 at slot 0 and selfdestructs on any call
    let pending = provider
        .send_transaction(deploy_tx(signer.address(), 0, selfdestruct_contract_init_code()))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment should succeed");

    let contract_address = receipt.contract_address.expect("Should have contract address");

    // Consume the canonical notification for deployment block
    let _ = node.canonical_stream.next().await;

    // Trigger SELFDESTRUCT by calling the contract
    let pending = provider.send_transaction(call_tx(signer.address(), contract_address, 1)).await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Selfdestruct tx should succeed");

    // Get the canonical notification for the selfdestruct block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: pre-Dencun, account MUST be destroyed
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    assert!(
        account_state.is_some_and(|a: &BundleAccount| a.was_destroyed()),
        "Pre-Dencun: Account MUST be marked as destroyed in output state"
    );

    // Verify via RPC that code and storage are cleared
    let code_after = provider.get_code_at(contract_address).await?;
    assert!(code_after.is_empty(), "Pre-Dencun: Contract code should be deleted");

    let slot0_after = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_after, U256::ZERO, "Pre-Dencun: Storage should be cleared");

    // Send ETH to the destroyed contract address in a new block.
    // This tests cache behavior - the cache should correctly reflect the account was destroyed.
    // Pre-Dencun: the contract no longer exists, so this is just a plain ETH transfer.
    let pending = provider
        .send_transaction(transfer_tx(signer.address(), contract_address, 2, U256::from(1000)))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "ETH transfer to destroyed contract address should succeed");

    // Consume the canonical notification
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state shows the account exists (received ETH) but has no code
    let account_state: Option<&BundleAccount> = execution_outcome.bundle.account(&contract_address);
    // After receiving ETH, the account should exist with balance but no code
    assert!(
        account_state.is_some(),
        "Pre-Dencun: Account should exist after receiving ETH (even though contract was destroyed)"
    );

    // Verify code is still empty (contract was destroyed, only ETH was received)
    let code_final = provider.get_code_at(contract_address).await?;
    assert!(code_final.is_empty(), "Pre-Dencun: Contract code should remain deleted");

    // Verify storage is still cleared
    let slot0_final = provider.get_storage_at(contract_address, U256::ZERO).await?;
    assert_eq!(slot0_final, U256::ZERO, "Pre-Dencun: Storage should remain cleared");

    // Verify the account now has the ETH balance we sent
    let balance = provider.get_balance(contract_address).await?;
    assert_eq!(balance, U256::from(1000), "Pre-Dencun: Account should have received ETH");

    Ok(())
}

/// Tests SELFDESTRUCT in same transaction as creation, where account previously had ETH
/// (post-Dencun).
///
/// Post-Dencun (EIP-6780):
/// - The same-tx exception applies when the CONTRACT is created in that transaction
/// - Even if the address previously had ETH (as an EOA), deploying a contract there and
///   selfdestructing in the same tx DOES delete the contract
/// - The "created in same tx" refers to contract creation, not account existence
///
/// This test verifies:
/// 1. Send ETH to the future contract address (address has balance but no code)
/// 2. Deploy contract that selfdestructs during constructor to that address
/// 3. Contract is deleted (same-tx exception applies - contract was created this tx)
/// 4. Code and storage are cleared
/// 5. Since account existed in DB before (had ETH), bundle marks it as Destroyed
#[tokio::test]
async fn test_selfdestruct_same_tx_preexisting_account_post_dencun() -> eyre::Result<()> {
    reth_tracing::init_test_tracing();

    let tree_config = TreeConfig::default().without_prewarming(true).without_state_cache(false);
    let (mut nodes, wallet) =
        setup_engine::<EthereumNode>(1, cancun_spec(), false, tree_config, eth_payload_attributes)
            .await?;
    let mut node = nodes.pop().unwrap();
    let signer = wallet.inner.clone();
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::new(signer.clone()))
        .connect_http(node.rpc_url());

    // Calculate where the contract will be deployed (CREATE uses sender + nonce)
    // We'll use nonce 1 for deployment, so first send ETH with nonce 0
    let future_contract_address = signer.address().create(1);

    // Send ETH to the future contract address first (makes it a pre-existing account)
    let pending = provider
        .send_transaction(transfer_tx(
            signer.address(),
            future_contract_address,
            0,
            U256::from(1000),
        ))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "ETH transfer should succeed");

    // Consume the canonical notification
    let _ = node.canonical_stream.next().await;

    // Verify the account exists and has balance
    let balance_before = provider.get_balance(future_contract_address).await?;
    assert_eq!(balance_before, U256::from(1000), "Account should have ETH before deployment");

    // Now deploy contract that selfdestructs during its constructor to the same address
    let pending = provider
        .send_transaction(deploy_tx(signer.address(), 1, selfdestruct_in_constructor_init_code()))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "Contract deployment with selfdestruct should succeed");

    // Verify deployment went to the expected address
    assert_eq!(
        receipt.contract_address,
        Some(future_contract_address),
        "Contract should be deployed to pre-computed address"
    );

    // Get the canonical notification for the deployment block
    let notification = node.canonical_stream.next().await.unwrap();
    let chain = notification.committed();
    let execution_outcome = chain.execution_outcome();

    // Verify the output state: same-tx exception DOES apply because contract was created this tx
    // The account should be marked as destroyed. Since it had prior state (ETH balance),
    // the bundle will contain it with status Destroyed and original_info set.
    let account_state: Option<&BundleAccount> =
        execution_outcome.bundle.account(&future_contract_address);
    assert!(
        account_state.is_some_and(|a| a.was_destroyed()),
        "Post-Dencun same-tx with prior ETH: Account MUST be marked as destroyed"
    );

    // Verify via RPC that code and storage are cleared
    let code = provider.get_code_at(future_contract_address).await?;
    assert!(code.is_empty(), "Post-Dencun same-tx: Contract code should be deleted");

    let slot0 = provider.get_storage_at(future_contract_address, U256::ZERO).await?;
    assert_eq!(slot0, U256::ZERO, "Post-Dencun same-tx: Storage should be cleared");

    // Balance should be zero (sent to beneficiary during SELFDESTRUCT)
    let balance_after = provider.get_balance(future_contract_address).await?;
    assert_eq!(
        balance_after,
        U256::ZERO,
        "Post-Dencun same-tx: Balance should be zero (sent to beneficiary)"
    );

    // Send ETH to the destroyed address to verify cache behavior
    let pending = provider
        .send_transaction(transfer_tx(
            signer.address(),
            future_contract_address,
            2,
            U256::from(2000),
        ))
        .await?;
    node.advance_block().await?;
    let receipt = pending.get_receipt().await?;
    assert!(receipt.status(), "ETH transfer should succeed");

    // Consume notification
    let _ = node.canonical_stream.next().await;

    // Verify the account received ETH and has no code (it's now just an EOA)
    let balance_final = provider.get_balance(future_contract_address).await?;
    assert_eq!(balance_final, U256::from(2000), "Account should have received ETH");

    let code_final = provider.get_code_at(future_contract_address).await?;
    assert!(code_final.is_empty(), "Code should remain empty after ETH transfer");

    let slot0_final = provider.get_storage_at(future_contract_address, U256::ZERO).await?;
    assert_eq!(slot0_final, U256::ZERO, "Storage should remain cleared");

    Ok(())
}
