use alloy_primitives::{Address, B256, U256};

pub const INTERNAL_TX_START_BLOCK_METHOD_ID: [u8; 4] = [0x6b, 0xf6, 0xa4, 0x2d];
pub const INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID: [u8; 4] = [0xb6, 0x69, 0x37, 0x71];

#[derive(Debug, Clone)]
pub struct InternalTxStartBlockData {
    pub l1_base_fee: U256,
    pub l1_block_number: u64,
    pub l2_block_number: u64,
    pub time_passed: u64,
}

#[derive(Debug, Clone)]
pub struct InternalTxBatchPostingReportData {
    pub batch_timestamp: U256,
    pub batch_poster_address: Address,
    pub batch_number: u64,
    pub batch_data_gas: u64,
    pub l1_base_fee_wei: U256,
}

pub fn unpack_internal_tx_data_start_block(data: &[u8]) -> Result<InternalTxStartBlockData, String> {
    if data.len() < 4 {
        return Err("Data too short for method selector".to_string());
    }
    
    let selector = &data[0..4];
    if selector != INTERNAL_TX_START_BLOCK_METHOD_ID {
        return Err(format!("Invalid method selector: expected {:?}, got {:?}", INTERNAL_TX_START_BLOCK_METHOD_ID, selector));
    }
    
    if data.len() < 132 {
        return Err(format!("Data too short: expected at least 132 bytes, got {}", data.len()));
    }
    
    let l1_base_fee_bytes = &data[4..36];
    let l1_block_number_bytes = &data[36..68];
    let l2_block_number_bytes = &data[68..100];
    let time_passed_bytes = &data[100..132];
    
    let l1_base_fee = U256::from_be_slice(l1_base_fee_bytes);
    let l1_block_number = U256::from_be_slice(l1_block_number_bytes).to::<u64>();
    let l2_block_number = U256::from_be_slice(l2_block_number_bytes).to::<u64>();
    let time_passed = U256::from_be_slice(time_passed_bytes).to::<u64>();
    
    Ok(InternalTxStartBlockData {
        l1_base_fee,
        l1_block_number,
        l2_block_number,
        time_passed,
    })
}

pub fn unpack_internal_tx_data_batch_posting_report(data: &[u8]) -> Result<InternalTxBatchPostingReportData, String> {
    if data.len() < 4 {
        return Err("Data too short for method selector".to_string());
    }
    
    let selector = &data[0..4];
    if selector != INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID {
        return Err(format!("Invalid method selector: expected {:?}, got {:?}", INTERNAL_TX_BATCH_POSTING_REPORT_METHOD_ID, selector));
    }
    
    if data.len() < 164 {
        return Err(format!("Data too short: expected at least 164 bytes, got {}", data.len()));
    }
    
    let batch_timestamp_bytes = &data[4..36];
    let batch_poster_address_bytes = &data[36..68];
    let batch_number_bytes = &data[68..100];
    let batch_data_gas_bytes = &data[100..132];
    let l1_base_fee_wei_bytes = &data[132..164];
    
    let batch_timestamp = U256::from_be_slice(batch_timestamp_bytes);
    let batch_poster_address = Address::from_slice(&batch_poster_address_bytes[12..32]);
    let batch_number = U256::from_be_slice(batch_number_bytes).to::<u64>();
    let batch_data_gas = U256::from_be_slice(batch_data_gas_bytes).to::<u64>();
    let l1_base_fee_wei = U256::from_be_slice(l1_base_fee_wei_bytes);
    
    Ok(InternalTxBatchPostingReportData {
        batch_timestamp,
        batch_poster_address,
        batch_number,
        batch_data_gas,
        l1_base_fee_wei,
    })
}
