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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_core_receipt_with_status_and_cumulative_gas() {
        let logs: Vec<Log> = Vec::new();
        let r = ArbRethReceiptBuilder::core_receipt(true, 12345, logs);
        match r.status {
            Eip658Value::Eip658(s) => assert!(s),
            _ => panic!("expected EIP-658 status"),
        }
        assert_eq!(r.cumulative_gas_used, 12345);
        assert!(r.logs.is_empty());
    }
}
