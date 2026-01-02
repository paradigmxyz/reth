//! Utilities helpers to create era data structures for testing purposes.

use crate::{
    era::types::consensus::{CompressedBeaconState, CompressedSignedBeaconBlock},
    era1::types::execution::{
        BlockTuple, CompressedBody, CompressedHeader, CompressedReceipts, TotalDifficulty,
    },
};
use alloy_consensus::{Header, ReceiptWithBloom};
use alloy_primitives::{Address, BlockNumber, Bytes, Log, LogData, B256, B64, U256};
use reth_ethereum_primitives::{Receipt, TxType};

// Helper function to create a test header
pub(crate) fn create_header() -> Header {
    Header {
        parent_hash: B256::default(),
        ommers_hash: B256::default(),
        beneficiary: Address::default(),
        state_root: B256::default(),
        transactions_root: B256::default(),
        receipts_root: B256::default(),
        logs_bloom: Default::default(),
        difficulty: U256::from(123456u64),
        number: 100,
        gas_limit: 5000000,
        gas_used: 21000,
        timestamp: 1609459200,
        extra_data: Bytes::default(),
        mix_hash: B256::default(),
        nonce: B64::default(),
        base_fee_per_gas: Some(10),
        withdrawals_root: Some(B256::default()),
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        #[cfg(feature = "amsterdam")]
        block_access_list_hash: None,
    }
}

// Helper function to create a test receipt with customizable parameters
pub(crate) fn create_test_receipt(
    tx_type: TxType,
    success: bool,
    cumulative_gas_used: u64,
    log_count: usize,
) -> Receipt {
    let mut logs = Vec::new();

    for i in 0..log_count {
        let address_byte = (i + 1) as u8;
        let topic_byte = (i + 10) as u8;
        let data_byte = (i + 100) as u8;

        logs.push(Log {
            address: Address::from([address_byte; 20]),
            data: LogData::new_unchecked(
                vec![B256::from([topic_byte; 32]), B256::from([topic_byte + 1; 32])],
                alloy_primitives::Bytes::from(vec![data_byte, data_byte + 1, data_byte + 2]),
            ),
        });
    }

    Receipt { tx_type, success, cumulative_gas_used, logs }
}

// Helper function to create a list of test receipts with different characteristics
pub(crate) fn create_test_receipts() -> Vec<Receipt> {
    vec![
        // Legacy transaction, successful, no logs
        create_test_receipt(TxType::Legacy, true, 21000, 0),
        // EIP-2930 transaction, failed, one log
        create_test_receipt(TxType::Eip2930, false, 42000, 1),
        // EIP-1559 transaction, successful, multiple logs
        create_test_receipt(TxType::Eip1559, true, 63000, 3),
        // EIP-4844 transaction, successful, two logs
        create_test_receipt(TxType::Eip4844, true, 84000, 2),
        // EIP-7702 transaction, failed, no logs
        create_test_receipt(TxType::Eip7702, false, 105000, 0),
    ]
}

pub(crate) fn create_test_receipt_with_bloom(
    tx_type: TxType,
    success: bool,
    cumulative_gas_used: u64,
    log_count: usize,
) -> ReceiptWithBloom {
    let receipt = create_test_receipt(tx_type, success, cumulative_gas_used, log_count);
    ReceiptWithBloom { receipt: receipt.into(), logs_bloom: Default::default() }
}

// Helper function to create a sample block tuple
pub(crate) fn create_sample_block(data_size: usize) -> BlockTuple {
    // Create a compressed header with very sample data - not compressed for simplicity
    let header_data = vec![0xAA; data_size];
    let header = CompressedHeader::new(header_data);

    // Create a compressed body with very sample data - not compressed for simplicity
    let body_data = vec![0xBB; data_size * 2];
    let body = CompressedBody::new(body_data);

    // Create compressed receipts with very sample data - not compressed for simplicity
    let receipts_data = vec![0xCC; data_size];
    let receipts = CompressedReceipts::new(receipts_data);

    let difficulty = TotalDifficulty::new(U256::from(data_size));

    // Create and return the block tuple
    BlockTuple::new(header, body, receipts, difficulty)
}

// Helper function to create a test block with compressed data
pub(crate) fn create_test_block_with_compressed_data(number: BlockNumber) -> BlockTuple {
    use alloy_consensus::{BlockBody, Header};
    use alloy_eips::eip4895::Withdrawals;
    use alloy_primitives::{Address, Bytes, B256, B64, U256};

    // Create test header
    let header = Header {
        parent_hash: B256::default(),
        ommers_hash: B256::default(),
        beneficiary: Address::default(),
        state_root: B256::default(),
        transactions_root: B256::default(),
        receipts_root: B256::default(),
        logs_bloom: Default::default(),
        difficulty: U256::from(number * 1000),
        number,
        gas_limit: 5000000,
        gas_used: 21000,
        timestamp: 1609459200 + number,
        extra_data: Bytes::default(),
        mix_hash: B256::default(),
        nonce: B64::default(),
        base_fee_per_gas: Some(10),
        withdrawals_root: Some(B256::default()),
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        #[cfg(feature = "amsterdam")]
        block_access_list_hash: None,
    };

    // Create test body
    let body: BlockBody<Bytes> = BlockBody {
        transactions: vec![Bytes::from(vec![(number % 256) as u8; 10])],
        ommers: vec![],
        withdrawals: Some(Withdrawals(vec![])),
    };

    // Create test receipt list with bloom
    let receipts_list: Vec<ReceiptWithBloom> = vec![create_test_receipt_with_bloom(
        reth_ethereum_primitives::TxType::Legacy,
        true,
        21000,
        0,
    )];

    // Compressed test compressed
    let compressed_header = CompressedHeader::from_header(&header).unwrap();
    let compressed_body = CompressedBody::from_body(&body).unwrap();
    let compressed_receipts = CompressedReceipts::from_encodable_list(&receipts_list).unwrap();
    let total_difficulty = TotalDifficulty::new(U256::from(number * 1000));

    BlockTuple::new(compressed_header, compressed_body, compressed_receipts, total_difficulty)
}

/// Helper function to create a simple beacon block
pub(crate) fn create_beacon_block(data_size: usize) -> CompressedSignedBeaconBlock {
    let block_data = vec![0xAA; data_size];
    CompressedSignedBeaconBlock::new(block_data)
}

/// Helper function to create a simple beacon state
pub(crate) fn create_beacon_state(data_size: usize) -> CompressedBeaconState {
    let state_data = vec![0xBB; data_size];
    CompressedBeaconState::new(state_data)
}
