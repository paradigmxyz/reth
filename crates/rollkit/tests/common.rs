//! Common test utilities and fixtures for rollkit tests.
//!
//! This module provides shared test setup, fixtures, and helper functions
//! to eliminate code duplication across different test files.

use std::sync::Arc;

use alloy_consensus::{TxLegacy, TypedTransaction};
use alloy_primitives::{Address, B256, Bytes, ChainId, Signature, TxKind, U256};
use eyre::Result;
use reth_chainspec::{MAINNET, ChainSpecBuilder};
use reth_ethereum_primitives::TransactionSigned;
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives::{Header, Transaction};
use reth_provider::test_utils::{MockEthProvider, ExtendedAccount};
use alloy_consensus::transaction::SignerRecoverable;
use tempfile::TempDir;

use rollkit_reth::{RollkitPayloadBuilder, RollkitPayloadAttributes};

// Test constants
pub const TEST_CHAIN_ID: u64 = 1234;
pub const GENESIS_HASH: &str = "0x2b8bbb1ea1e04f9c9809b4b278a8687806edc061a356c7dbc491930d8e922503";
pub const GENESIS_STATEROOT: &str = "0x05e9954443da80d86f2104e56ffdfd98fe21988730684360104865b3dc8191b4";
pub const TEST_TO_ADDRESS: &str = "0x944fDcD1c868E3cC566C78023CcB38A32cDA836E";
pub const TEST_TIMESTAMP: u64 = 1710338135;
pub const TEST_GAS_LIMIT: u64 = 30_000_000;

/// Shared test fixture for rollkit payload builder tests
pub struct RollkitTestFixture {
    pub builder: RollkitPayloadBuilder<MockEthProvider>,
    pub provider: MockEthProvider,
    pub genesis_hash: B256,
    pub genesis_state_root: B256,
    #[allow(dead_code)]
    pub temp_dir: TempDir,
}

impl RollkitTestFixture {
    /// Creates a new test fixture with mock provider and genesis state
    pub async fn new() -> Result<Self> {
        let temp_dir = tempfile::tempdir()?;
        let provider = MockEthProvider::default();
        
        let genesis_hash = B256::from_slice(&hex::decode(&GENESIS_HASH[2..]).unwrap());
        let genesis_state_root = B256::from_slice(&hex::decode(&GENESIS_STATEROOT[2..]).unwrap());
        
        // Setup genesis header with all required fields for modern Ethereum
        let mut genesis_header = Header::default();
        genesis_header.state_root = genesis_state_root;
        genesis_header.number = 0;
        genesis_header.gas_limit = TEST_GAS_LIMIT;
        genesis_header.timestamp = TEST_TIMESTAMP;
        // EIP-4844 fields required for Cancun and later hardforks
        genesis_header.excess_blob_gas = Some(0);
        genesis_header.blob_gas_used = Some(0);
        // EIP-4788 field required for Cancun and later hardforks
        genesis_header.parent_beacon_block_root = Some(B256::ZERO);
        
        provider.add_header(genesis_hash, genesis_header);
        
        // Create a test chain spec with our test chain ID
        let test_chainspec = ChainSpecBuilder::from(&*MAINNET)
            .chain(reth_chainspec::Chain::from_id(TEST_CHAIN_ID))
            .cancun_activated()
            .build();
        let evm_config = EthEvmConfig::new(Arc::new(test_chainspec));
        
        let builder = RollkitPayloadBuilder::new(Arc::new(provider.clone()), evm_config);
        
        let fixture = Self {
            builder,
            provider,
            genesis_hash,
            genesis_state_root,
            temp_dir,
        };
        
        fixture.setup_test_accounts();
        Ok(fixture)
    }

    /// Setup test accounts with sufficient balances
    pub fn setup_test_accounts(&self) {
        let account = ExtendedAccount::new(0, U256::from(1000_u64) * U256::from(1_000_000_000_000_000_000u64));
        
        // Find which address the test signature resolves to
        let test_signed = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy {
                chain_id: Some(ChainId::from(TEST_CHAIN_ID)),
                nonce: 0,
                gas_price: 0,
                gas_limit: 21_000,
                to: TxKind::Call(Address::ZERO),
                value: U256::ZERO,
                input: Bytes::default(),
            }), 
            Signature::test_signature()
        );
        
        if let Ok(recovered) = test_signed.recover_signer() {
            self.provider.add_account(recovered, account);
        }
    }

    /// Adds a mock header to the provider for proper parent lookups
    pub fn add_mock_header(&self, hash: B256, number: u64, state_root: B256, timestamp: u64) {
        let mut header = Header::default();
        header.number = number;
        header.state_root = state_root;
        header.gas_limit = TEST_GAS_LIMIT;
        header.timestamp = timestamp;
        header.excess_blob_gas = Some(0);
        header.blob_gas_used = Some(0);
        header.parent_beacon_block_root = Some(B256::ZERO);
        
        self.provider.add_header(hash, header);
    }

    /// Creates payload attributes for testing
    pub fn create_payload_attributes(
        &self,
        transactions: Vec<TransactionSigned>,
        block_number: u64,
        timestamp: u64,
        parent_hash: B256,
        gas_limit: Option<u64>,
    ) -> RollkitPayloadAttributes {
        RollkitPayloadAttributes::new(
            transactions,
            gas_limit,
            timestamp,
            B256::random(), // prev_randao
            Address::random(), // suggested_fee_recipient
            parent_hash,
            block_number,
        )
    }
}

/// Creates test transactions with specified count and starting nonce
pub fn create_test_transactions(count: usize, nonce_start: u64) -> Vec<TransactionSigned> {
    let mut transactions = Vec::with_capacity(count);
    let to_address = Address::from_slice(&hex::decode(&TEST_TO_ADDRESS[2..]).unwrap());
    
    for i in 0..count {
        let nonce = nonce_start + i as u64;
        
        let legacy_tx = TxLegacy {
            chain_id: Some(ChainId::from(TEST_CHAIN_ID)),
            nonce,
            gas_price: 0, // Zero gas price for testing
            gas_limit: 21_000,
            to: TxKind::Call(to_address),
            value: U256::from(0), // No value transfer
            input: Bytes::default(),
        };

        let typed_tx = TypedTransaction::Legacy(legacy_tx);
        let transaction = Transaction::from(typed_tx);
        let signed_tx = TransactionSigned::new_unhashed(transaction, Signature::test_signature());
        transactions.push(signed_tx);
    }
    
    transactions
}

/// Creates a single test transaction with specified nonce
pub fn create_test_transaction(nonce: u64) -> TransactionSigned {
    create_test_transactions(1, nonce).into_iter().next().unwrap()
} 