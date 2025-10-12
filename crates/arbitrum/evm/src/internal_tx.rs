use alloy_primitives::{Address, U256, B256, Bytes};
use reth_arbitrum_primitives::{ArbTransactionSigned, ArbTypedTransaction};
use crate::execute::ArbTxProcessorState;
use arb_alloy_consensus::tx::ArbInternalTx;

const INTERNAL_TX_START_BLOCK_METHOD_ID: [u8; 4] = [0x6b, 0xf6, 0xa4, 0x2d];
const INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID: [u8; 4] = [0x0a, 0xdc, 0x77, 0x7d];
const INTERNAL_TX_BATCH_POSTING_REPORT_V2_METHOD_ID: [u8; 4] = [0xf1, 0x9a, 0xdd, 0xcc];

pub fn create_internal_tx_start_block(
    chain_id: U256,
    l1_base_fee: U256,
    l1_block_num: u64,
    l2_block_num: u64,
    time_passed: u64,
) -> ArbTransactionSigned {
    // Pack the data: method_id + l1_base_fee + l1_block_num + l2_block_num + time_passed
    let mut data = Vec::with_capacity(4 + 32 * 4);
    
    data.extend_from_slice(&INTERNAL_TX_START_BLOCK_METHOD_ID);
    
    // l1_base_fee (32 bytes, big-endian)
    let mut l1_base_fee_bytes = [0u8; 32];
    l1_base_fee.to_be_bytes_trimmed_vec().iter().rev().enumerate()
        .for_each(|(i, &b)| l1_base_fee_bytes[31 - i] = b);
    data.extend_from_slice(&l1_base_fee_bytes);
    
    // l1_block_num (32 bytes, big-endian, right-aligned)
    let mut l1_block_num_bytes = [0u8; 32];
    l1_block_num_bytes[24..32].copy_from_slice(&l1_block_num.to_be_bytes());
    data.extend_from_slice(&l1_block_num_bytes);
    
    let mut l2_block_num_bytes = [0u8; 32];
    l2_block_num_bytes[24..32].copy_from_slice(&l2_block_num.to_be_bytes());
    data.extend_from_slice(&l2_block_num_bytes);
    
    // time_passed (32 bytes, big-endian, right-aligned)
    let mut time_passed_bytes = [0u8; 32];
    time_passed_bytes[24..32].copy_from_slice(&time_passed.to_be_bytes());
    data.extend_from_slice(&time_passed_bytes);
    
    let internal_tx = ArbInternalTx {
        chain_id,
        data: Bytes::from(data),
    };
    
    ArbTransactionSigned::new_unhashed(
        ArbTypedTransaction::Internal(internal_tx),
        alloy_primitives::Signature::test_signature(),
    )
}

pub fn apply_internal_tx_update<D>(
    db: &mut revm::database::State<D>,
    state: &mut ArbTxProcessorState,
    tx: &ArbTransactionSigned,
) -> Result<(), String>
where
    D: revm::Database,
{
    use alloy_consensus::Transaction;
    let tx_data = tx.input();
    
    if tx_data.len() < 4 {
        return Err(format!("internal tx data is too short (only {} bytes, at least 4 required)", tx_data.len()));
    }
    
    let selector: [u8; 4] = [tx_data[0], tx_data[1], tx_data[2], tx_data[3]];
    
    tracing::info!(
        "Internal tx selector: {:02x}{:02x}{:02x}{:02x}, data length: {}, full_data: {}",
        selector[0], selector[1], selector[2], selector[3],
        tx_data.len(),
        hex::encode(tx_data)
    );
    
    match selector {
        INTERNAL_TX_START_BLOCK_METHOD_ID => {
            tracing::debug!("Processing StartBlock internal tx");
            apply_start_block(db, state, tx_data)
        }
        INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID => {
            tracing::debug!("Processing BatchPostingReport internal tx");
            apply_batch_posting_report(db, state, tx_data)
        }
        INTERNAL_TX_BATCH_POSTING_REPORT_V2_METHOD_ID => {
            tracing::debug!("Processing BatchPostingReportV2 internal tx");
            apply_batch_posting_report_v2(db, state, tx_data)
        }
        _ => {
            Err(format!("unknown internal tx method selector: {:02x}{:02x}{:02x}{:02x}", 
                selector[0], selector[1], selector[2], selector[3]))
        }
    }
}

fn apply_start_block<D>(
    db: &mut revm::database::State<D>,
    state: &mut ArbTxProcessorState,
    tx_data: &[u8],
) -> Result<(), String>
where
    D: revm::Database,
{
    if tx_data.len() < 4 + 32 + 32 + 32 + 32 {
        return Err("internal tx data too short for StartBlock".to_string());
    }
    
    let mut offset = 4;
    
    let l1_base_fee = U256::from_be_slice(&tx_data[offset..offset + 32]);
    offset += 32;
    
    let l1_block_number = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _l2_block_number = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let time_passed = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    
    tracing::debug!(
        "InternalTxStartBlock: l1_base_fee={}, l1_block_number={}, time_passed={}",
        l1_base_fee,
        l1_block_number,
        time_passed
    );
    
    update_l1_block_number_in_state(db, l1_block_number).map_err(|e| format!("Failed to update L1 block number: {}", e))?;
    
    state.l1_base_fee = l1_base_fee;
    
    tracing::info!("InternalTxStartBlock: Updated ArbOS state with L1 block number {}", l1_block_number);
    
    Ok(())
}

fn update_l1_block_number_in_state<D>(
    db: &mut revm::database::State<D>,
    l1_block_number: u64,
) -> Result<(), String>
where
    D: revm::Database,
{
    use crate::header::arbos_l1_block_number_slot;
    use revm_state::{AccountInfo, EvmStorageSlot};
    use revm_database::{BundleAccount, AccountStatus};
    use revm::Database;
    use std::collections::HashMap;
    
    let (addr, slot) = arbos_l1_block_number_slot();
    
    let value = alloy_primitives::U256::from(l1_block_number);
    
    if !db.bundle_state.state.contains_key(&addr) {
        let info = match db.basic(addr) {
            Ok(Some(account_info)) => Some(account_info),
            _ => Some(AccountInfo {
                balance: alloy_primitives::U256::ZERO,
                nonce: 0,
                code_hash: alloy_primitives::keccak256([]),
                code: None,
            }),
        };
        let acc = BundleAccount {
            info,
            storage: HashMap::default(),
            original_info: None,
            status: AccountStatus::Changed,
        };
        db.bundle_state.state.insert(addr, acc);
    }
    
    let slot_u256 = alloy_primitives::U256::from_be_bytes(slot.0);
    
    if let Some(acc) = db.bundle_state.state.get_mut(&addr) {
        acc.storage.insert(
            slot_u256,
            EvmStorageSlot { present_value: value, ..Default::default() }.into(),
        );
    }
    
    tracing::debug!("Set ArbOS L1 block number to {} at addr={:?} slot={:?}", l1_block_number, addr, slot);
    
    Ok(())
}

fn apply_batch_posting_report<D>(
    _db: &mut revm::database::State<D>,
    _state: &mut ArbTxProcessorState,
    tx_data: &[u8],
) -> Result<(), String>
where
    D: revm::Database,
{
    if tx_data.len() < 4 + 32 + 32 + 32 + 32 + 32 {
        return Err("internal tx data too short for BatchPostingReport".to_string());
    }
    
    let mut offset = 4;
    
    let _batch_timestamp = U256::from_be_slice(&tx_data[offset..offset + 32]);
    offset += 32;
    
    let _batch_poster_address = Address::from_slice(&tx_data[offset + 12..offset + 32]);
    offset += 32;
    
    let _batch_number = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _batch_data_gas = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _l1_base_fee_wei = U256::from_be_slice(&tx_data[offset..offset + 32]);
    
    tracing::debug!("InternalTxBatchPostingReport processed");
    
    Ok(())
}

fn apply_batch_posting_report_v2<D>(
    _db: &mut revm::database::State<D>,
    _state: &mut ArbTxProcessorState,
    tx_data: &[u8],
) -> Result<(), String>
where
    D: revm::Database,
{
    if tx_data.len() < 4 + 32 + 32 + 32 + 32 + 32 + 32 + 32 {
        return Err("internal tx data too short for BatchPostingReportV2".to_string());
    }
    
    let mut offset = 4;
    
    let _batch_timestamp = U256::from_be_slice(&tx_data[offset..offset + 32]);
    offset += 32;
    
    let _batch_poster_address = Address::from_slice(&tx_data[offset + 12..offset + 32]);
    offset += 32;
    
    let _batch_number = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _batch_calldata_length = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _batch_calldata_non_zeros = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _batch_extra_gas = u64::from_be_bytes([
        tx_data[offset + 24], tx_data[offset + 25], tx_data[offset + 26], tx_data[offset + 27],
        tx_data[offset + 28], tx_data[offset + 29], tx_data[offset + 30], tx_data[offset + 31],
    ]);
    offset += 32;
    
    let _l1_base_fee_wei = U256::from_be_slice(&tx_data[offset..offset + 32]);
    
    tracing::debug!("InternalTxBatchPostingReportV2 processed");
    
    Ok(())
}
