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

use alloy_evm::eth::receipt_builder::ReceiptBuilderCtx;
use reth_evm::Evm;

pub trait ArbReceiptBuilder {
    type Transaction;
    type Receipt;

    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, Self::Transaction, E>>;
}

impl ArbReceiptBuilder for ArbRethReceiptBuilder {
    type Transaction = ();
    type Receipt = AlloyReceipt;

    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, Self::Transaction, E>> {
        let receipt = AlloyReceipt {
            status: Eip658Value::Eip658(ctx.result.is_success()),
            cumulative_gas_used: ctx.cumulative_gas_used,
            logs: ctx.result.into_logs(),
        };
        Ok(receipt)
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
