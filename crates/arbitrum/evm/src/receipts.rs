extern crate alloc;

use alloc::vec::Vec;
use alloy_consensus::Eip658Value;
use reth_primitives::{Log, Receipt, ReceiptWithBloom};

pub struct ArbRethReceiptBuilder;

impl ArbRethReceiptBuilder {
    pub fn from_eip658(status: bool, cumulative_gas_used: u64, logs: Vec<Log>) -> Receipt {
        Receipt::new(Eip658Value::Eip658(status), cumulative_gas_used, logs)
    }

    pub fn with_bloom(receipt: Receipt) -> ReceiptWithBloom {
        ReceiptWithBloom::from(receipt)
    }
}
