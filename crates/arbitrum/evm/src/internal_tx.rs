use alloy_primitives::{B256, U256};

pub const INTERNAL_TX_START_BLOCK_METHOD_ID: [u8; 4] = [0x6b, 0xf6, 0xa4, 0x2d];

#[derive(Debug, Clone)]
pub struct InternalTxStartBlockData {
    pub l1_base_fee: U256,
    pub l1_block_number: u64,
    pub l2_block_number: u64,
    pub time_passed: u64,
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
