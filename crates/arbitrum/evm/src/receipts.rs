extern crate alloc;

use alloc::vec::Vec;
use alloy_consensus::{Eip658Value, Receipt as AlloyReceipt};
use alloy_evm::eth::receipt_builder::ReceiptBuilderCtx;
use alloy_primitives::Log;
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

    fn build_receipt<'a, E>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, Self::Transaction, E>>;

    fn build_deposit_receipt(&self, inner: ArbDepositReceipt) -> Self::Receipt;
}

impl ArbReceiptBuilder for ArbRethReceiptBuilder {
    type Transaction = ArbTransactionSigned;
    type Receipt = ArbReceipt;

    fn build_receipt<'a, E>(
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
        let _ = ArbReceipt::Legacy(base);
    }

    #[test]
    fn deposit_receipt_build_path_errors() {
        struct DummyEvm;

        let builder = ArbRethReceiptBuilder::default();
        use arb_alloy_consensus::tx::ArbDepositTx;
        use alloy_primitives::{address, b256, U256, Signature};
        use reth_arbitrum_primitives::ArbTypedTransaction;
        let dep = ArbDepositTx {
            chain_id: U256::from(42161u64),
            l1_request_id: b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            from: address!("00000000000000000000000000000000000000aa"),
            to: address!("00000000000000000000000000000000000000bb"),
            value: U256::ZERO,
        };
        let tx = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Deposit(dep),
            Signature::default(),
        );
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
    fn non_deposit_tx_types_build_legacy_receipts_without_l1_fields() {
        struct DummyEvm;

        let builder = ArbRethReceiptBuilder::default();

        let base_result = alloy_evm::eth::receipt_builder::ExecutionResult {
            success: true,
            logs: Vec::<Log>::new(),
            return_value: alloy_primitives::Bytes::default(),
            gas_used: 0,
        };

        fn run_tx<E: Evm>(
            builder: &ArbRethReceiptBuilder,
            tx: &reth_arbitrum_primitives::ArbTransactionSigned,
        ) -> ArbReceipt {
            let mut evm = DummyEvm;
            let ctx = ReceiptBuilderCtx {
                tx,
                result: base_result.clone(),
                cumulative_gas_used: 21000,
                index: 0,
                evm: &mut evm,
            };
            builder.build_receipt::<DummyEvm>(ctx).expect("non-deposit should build")
        }

        use arb_alloy_consensus::tx::{ArbUnsignedTx, ArbContractTx, ArbRetryTx, ArbSubmitRetryableTx, ArbInternalTx};
        use alloy_primitives::{address, b256, U256, Signature};
        use reth_arbitrum_primitives::ArbTypedTransaction;

        let unsigned = ArbUnsignedTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000001"),
            nonce: 1,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: None,
            value: U256::ZERO,
            data: Vec::new(),
        };
        let tx_unsigned = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Unsigned(unsigned), Signature::default());
        let r = run_tx::<DummyEvm>(&builder, &tx_unsigned);
        match r { ArbReceipt::Legacy(rec) => {
            match rec.status { Eip658Value::Eip658(true) => {}, _ => panic!("expected EIP-658 status") }
        }, _ => panic!("expected Legacy receipt") }

        let contract = ArbContractTx {
            chain_id: U256::from(42161u64),
            nonce: 2,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000002")),
            value: U256::ZERO,
            data: Vec::new(),
        };
        let tx_contract = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Contract(contract), Signature::default());
        let _ = run_tx::<DummyEvm>(&builder, &tx_contract);

        let retry = ArbRetryTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000003"),
            nonce: 3,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000004")),
            value: U256::ZERO,
            data: Vec::new(),
            ticket_id: b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            refund_to: address!("0000000000000000000000000000000000000005"),
        };
        let tx_retry = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Retry(retry), Signature::default());
        let _ = run_tx::<DummyEvm>(&builder, &tx_retry);

        let srt = ArbSubmitRetryableTx {
            chain_id: U256::from(42161u64),
            request_id: b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            l1_base_fee: U256::from(1u64),
            deposit: U256::from(2u64),
            callvalue: U256::from(3u64),
            gas_fee_cap: U256::from(4u64),
            gas_limit: 21000,
            max_submission_fee: U256::from(5u64),
            fee_refund_address: address!("0000000000000000000000000000000000000006"),
            beneficiary: address!("0000000000000000000000000000000000000007"),
            to: address!("0000000000000000000000000000000000000008"),
            data: Vec::new(),
        };
        let tx_srt = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::SubmitRetryable(srt), Signature::default());
        let _ = run_tx::<DummyEvm>(&builder, &tx_srt);

        let itx = ArbInternalTx {
            chain_id: U256::from(42161u64),
            caller: address!("0000000000000000000000000000000000000009"),
            to: address!("000000000000000000000000000000000000000a"),
            gas: 100000,
            data: Vec::new(),
        };
        let tx_itx = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Internal(itx), Signature::default());
        let _ = run_tx::<DummyEvm>(&builder, &tx_itx);

        let leg = alloy_consensus::TxLegacy {
            chain_id: Some(42161),
            nonce: 0,
            gas_price: 1u128.into(),
            gas_limit: 21000,
            to: Some(address!("000000000000000000000000000000000000000b")),
            value: 0u128.into(),
            input: Vec::new().into(),
        };
        let tx_legacy = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Legacy(leg), Signature::default());
        let _ = run_tx::<DummyEvm>(&builder, &tx_legacy);
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
