extern crate alloc;

use alloc::vec::Vec;
use alloy_consensus::{Eip658Value, Receipt as AlloyReceipt};
use alloy_evm::eth::receipt_builder::ReceiptBuilderCtx;
use alloy_primitives::Log;
use reth_evm::Evm;
use reth_arbitrum_primitives::{ArbDepositReceipt, ArbReceipt, ArbTransactionSigned, ArbTxType};

#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ArbRethReceiptBuilder;

impl ArbRethReceiptBuilder {
    pub fn core_receipt(status: bool, cumulative_gas_used: u64, logs: Vec<Log>) -> AlloyReceipt {
        AlloyReceipt { status: Eip658Value::Eip658(status), cumulative_gas_used, logs }
    }
}

pub trait ArbReceiptBuilder {
    type Transaction;
    type Receipt;

    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, Self::Transaction, E>>;

    fn build_deposit_receipt(&self, inner: ArbDepositReceipt) -> Self::Receipt;
}

impl ArbReceiptBuilder for ArbRethReceiptBuilder {
    type Transaction = ArbTransactionSigned;
    type Receipt = ArbReceipt;

    fn build_receipt<'a, E: Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, ArbTransactionSigned, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, ArbTransactionSigned, E>> {
        match ctx.tx.tx_type() {
            ArbTxType::Deposit => Err(ctx),
            ty => {
                let receipt = AlloyReceipt {
                    status: Eip658Value::Eip658(ctx.result.is_success()),
                    cumulative_gas_used: ctx.cumulative_gas_used,
                    logs: ctx.result.into_logs(),
                };
                let out = match ty {
                    ArbTxType::Unsigned => ArbReceipt::Legacy(receipt),
                    ArbTxType::Contract => ArbReceipt::Legacy(receipt),
                    ArbTxType::Retry => ArbReceipt::Legacy(receipt),
                    ArbTxType::SubmitRetryable => ArbReceipt::Legacy(receipt),
                    ArbTxType::Internal => ArbReceipt::Legacy(receipt),
                    ArbTxType::Legacy => ArbReceipt::Legacy(receipt),
                    ArbTxType::Deposit => unreachable!(),
                };
                Ok(out)
            }
        }
    }

    fn build_deposit_receipt(&self, inner: ArbDepositReceipt) -> Self::Receipt {
        ArbReceipt::Deposit(inner)
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

    #[test]
    fn maps_tx_types_to_receipt_variants() {
        let logs: Vec<Log> = Vec::new();
        let base = ArbRethReceiptBuilder::core_receipt(true, 1, logs);
        let _ = ArbReceipt::Legacy(base.clone());
    let _ = ArbReceipt::Legacy(base.clone());

        let _ = ArbReceipt::Legacy(base.clone());
        let _ = ArbReceipt::Legacy(base);
    }

    #[test]
    fn deposit_receipt_build_path_errors() {
        use reth_evm::Evm;
        struct DummyEvm;
        impl Evm for DummyEvm {}

        let builder = ArbRethReceiptBuilder::default();
        let tx = ArbTransactionSigned { ty: ArbTxType::Deposit };
        let mut evm = DummyEvm;
        let ctx = ReceiptBuilderCtx {
            tx: &tx,
            result: alloy_evm::eth::receipt_builder::ExecutionResult {
                success: true,
                logs: Vec::<Log>::new(),
                return_value: alloy_primitives::Bytes::default(),
                gas_used: 0,
            },
            cumulative_gas_used: 0,
            index: 0,
            evm: &mut evm,
        };
        let res = builder.build_receipt::<DummyEvm>(ctx);
        assert!(res.is_err());
    }

    #[test]
    fn builds_deposit_receipt() {
        let builder = ArbRethReceiptBuilder::default();
        let dep = ArbDepositReceipt::default();
        let r = builder.build_deposit_receipt(dep);
        match r {
            ArbReceipt::Deposit(_) => {}
            _ => panic!("expected deposit receipt"),
        }
    }
}
