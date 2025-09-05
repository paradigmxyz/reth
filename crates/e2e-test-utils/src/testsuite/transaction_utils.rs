//! Transaction creation utilities for e2e tests.

use alloy_consensus::TxLegacy;
use alloy_eips::eip2718::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use eyre::Result;
use crate::wallet::Wallet;
use std::str::FromStr;

/// Create a transaction that calls approve() on a token contract.
/// This modifies storage in a way that can trigger trie node creation/deletion.
///
/// # Arguments
/// * `token_address` - Address of the token contract
/// * `spender` - Address to approve for spending
/// * `amount` - Amount to approve
/// * `nonce` - Transaction nonce
/// * `chain_id` - Chain ID for the transaction
pub async fn create_approve_tx(
    token_address: Address,
    spender: Address, 
    amount: U256, 
    nonce: u64,
    chain_id: u64
) -> Result<Bytes> {
    // The approve(address spender, uint256 amount) selector: 0x095ea7b3
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&[0x09, 0x5e, 0xa7, 0xb3]); // approve selector
    data.extend_from_slice(&[0u8; 12]); // padding for address
    data.extend_from_slice(spender.as_slice()); // spender address (20 bytes)
    
    // Encode amount as 32 bytes
    let amount_bytes = amount.to_be_bytes::<32>();
    data.extend_from_slice(&amount_bytes);
    
    // Get test wallet (funded account from genesis)
    let wallet = Wallet::new(1).with_chain_id(chain_id);
    let signers = wallet.wallet_gen();
    let signer = signers.into_iter().next()
        .ok_or_else(|| eyre::eyre!("Failed to create signer"))?;
    
    // Create transaction calling the contract
    let mut tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: 20_000_000_000, // 20 gwei
        gas_limit: 100_000,
        to: TxKind::Call(token_address),
        value: U256::ZERO, // No ETH sent
        input: data.into(),
    };
    
    // Sign the transaction
    let signature = signer.sign_transaction_sync(&mut tx)?;
    
    // Build the signed transaction
    let signed = alloy_consensus::TxEnvelope::Legacy(
        alloy_consensus::Signed::new_unchecked(tx, signature, alloy_primitives::B256::ZERO)
    );
    
    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);
    
    Ok(Bytes::from(encoded))
}

/// Create a simple transfer transaction that sends ETH.
///
/// # Arguments
/// * `to` - Recipient address
/// * `value` - Amount of ETH to send (in wei)
/// * `nonce` - Transaction nonce
/// * `chain_id` - Chain ID for the transaction
pub async fn create_transfer_tx(
    to: Address, 
    value: U256,
    nonce: u64,
    chain_id: u64
) -> Result<Bytes> {
    // Get test wallet (funded account from genesis)
    let wallet = Wallet::new(1).with_chain_id(chain_id);
    let signers = wallet.wallet_gen();
    let signer = signers.into_iter().next()
        .ok_or_else(|| eyre::eyre!("Failed to create signer"))?;
    
    // Create simple transfer transaction
    let mut tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: 20_000_000_000, // 20 gwei
        gas_limit: 21_000, // Standard gas for transfer
        to: TxKind::Call(to),
        value,
        input: Bytes::new(), // Empty data for simple transfer
    };
    
    // Sign the transaction
    let signature = signer.sign_transaction_sync(&mut tx)?;
    
    // Build the signed transaction
    let signed = alloy_consensus::TxEnvelope::Legacy(
        alloy_consensus::Signed::new_unchecked(tx, signature, alloy_primitives::B256::ZERO)
    );
    
    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);
    
    Ok(Bytes::from(encoded))
}

/// Helper to create approve transaction for the default test token contract.
/// The default contract address is 0x77d34361f991fa724ff1db9b1d760063a16770db
/// which is used in the trie corruption tests.
pub async fn create_test_approve_tx(
    spender: Address,
    amount: U256,
    nonce: u64,
    chain_id: u64
) -> Result<Bytes> {
    let token_address = Address::from_str("0x77d34361f991fa724ff1db9b1d760063a16770db")?;
    create_approve_tx(token_address, spender, amount, nonce, chain_id).await
}