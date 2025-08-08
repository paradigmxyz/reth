extern crate alloc;

use alloc::vec::Vec;
use alloy_consensus::{Eip658Value, Receipt as AlloyReceipt};
use alloy_primitives::Log;

pub struct ArbRethReceiptBuilder;

impl ArbRethReceiptBuilder {
    pub fn core_receipt(status: bool, cumulative_gas_used: u64, logs: Vec<Log>) -> AlloyReceipt {
        AlloyReceipt { status: Eip658Value::Eip658(status), cumulative_gas_used, logs }
    }
}
