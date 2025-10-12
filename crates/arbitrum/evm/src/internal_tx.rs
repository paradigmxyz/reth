use alloy_primitives::{Address, U256, B256};
use reth_arbitrum_primitives::ArbTransactionSigned;
use crate::execute::ArbTxProcessorState;

const INTERNAL_TX_START_BLOCK_METHOD_ID: [u8; 4] = [0xef, 0x5c, 0xc5, 0x56];
const INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID: [u8; 4] = [0x0a, 0xdc, 0x77, 0x7d];
const INTERNAL_TX_BATCH_POSTING_REPORT_V2_METHOD_ID: [u8; 4] = [0xf1, 0x9a, 0xdd, 0xcc];

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
    _db: &mut revm::database::State<D>,
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
    
    let _l1_base_fee = U256::from_be_slice(&tx_data[offset..offset + 32]);
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
        "InternalTxStartBlock: l1_block_number={}, time_passed={}",
        l1_block_number,
        time_passed
    );
    
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
