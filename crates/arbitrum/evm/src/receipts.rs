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

    fn build_receipt<'a, E: reth_evm::Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, Self::Transaction, E>>;

    fn build_deposit_receipt(&self, inner: ArbDepositReceipt) -> Self::Receipt;
}

impl ArbReceiptBuilder for ArbRethReceiptBuilder {
    type Transaction = ArbTransactionSigned;
    type Receipt = ArbReceipt;

    fn build_receipt<'a, E: reth_evm::Evm>(
        &self,
        ctx: ReceiptBuilderCtx<'a, ArbTransactionSigned, E>,
    ) -> Result<Self::Receipt, ReceiptBuilderCtx<'a, ArbTransactionSigned, E>> {
        match ctx.tx.tx_type() {
            ArbTxType::Deposit => Err(ctx),
            ty => {
                use alloy_eips::eip2718::Encodable2718;
                let tx_hash = {
                    let mut buf = Vec::new();
                    ctx.tx.encode_2718(&mut buf);
                    alloy_primitives::keccak256(&buf)
                };
                
                let status_flag = ctx.result.is_success();
                let gas_used = ctx.result.gas_used();
                
                let (actual_gas_used, cumulative_gas) = if let Some(early_gas) = crate::get_early_tx_gas(&tx_hash) {
                    let gas_diff = early_gas as i64 - gas_used as i64;
                    tracing::debug!(
                        target: "arb-reth::receipt",
                        tx_hash = ?tx_hash,
                        early_gas = early_gas,
                        evm_gas = gas_used,
                        gas_diff = gas_diff,
                        "Using early termination gas for receipt"
                    );
                    crate::clear_early_tx_gas(&tx_hash);
                    crate::add_gas_adjustment(gas_diff);
                    let cumulative = ctx.cumulative_gas_used - gas_used + early_gas;
                    (early_gas, cumulative)
                } else {
                    (gas_used, ctx.cumulative_gas_used)
                };
                
                let mut logs = ctx.result.into_logs();
                let mut extra = crate::log_sink::take();
                if !extra.is_empty() {
                    logs.append(&mut extra);
                }
                
                let receipt = AlloyReceipt {
                    status: Eip658Value::Eip658(status_flag),
                    cumulative_gas_used: cumulative_gas,
                    logs,
                };
                let out = match ty {
                    ArbTxType::Unsigned => ArbReceipt::Legacy(receipt),
                    ArbTxType::Contract => ArbReceipt::Legacy(receipt),
                    ArbTxType::Retry => ArbReceipt::Legacy(receipt),
                    ArbTxType::SubmitRetryable => ArbReceipt::Legacy(receipt),
                    ArbTxType::Internal => ArbReceipt::Legacy(receipt),
                    ArbTxType::Legacy => ArbReceipt::Legacy(receipt),
                    ArbTxType::Eip2930 => ArbReceipt::Legacy(receipt),
                    ArbTxType::Eip1559 => ArbReceipt::Legacy(receipt),
                    ArbTxType::Eip4844 => ArbReceipt::Legacy(receipt),
                    ArbTxType::Eip7702 => ArbReceipt::Legacy(receipt),
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
impl alloy_evm::eth::receipt_builder::ReceiptBuilder for ArbRethReceiptBuilder {
    type Transaction = ArbTransactionSigned;
    type Receipt = ArbReceipt;

    fn build_receipt<'a, E: reth_evm::Evm>(
        &self,
        ctx: alloy_evm::eth::receipt_builder::ReceiptBuilderCtx<'a, Self::Transaction, E>,
    ) -> Self::Receipt {
        match <Self as crate::receipts::ArbReceiptBuilder>::build_receipt::<E>(self, ctx) {
            Ok(r) => r,
            Err(_ctx) => self.build_deposit_receipt(ArbDepositReceipt::default()),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, Default)]
    struct StubEvm;

    impl reth_evm::Evm for StubEvm {
        type DB = ();
        type Tx = crate::arb_evm::ArbTransaction<revm::context::TxEnv>;
        type Error = revm::context::result::EVMError<reth_storage_errors::db::DatabaseError, revm::context::result::InvalidTransaction>;
        type HaltReason = revm::context::result::HaltReason;
        type Spec = crate::SpecId;
        type Precompiles = alloy_evm::precompiles::PrecompilesMap;
        type Inspector = ();

        fn block(&self) -> &revm::context::BlockEnv { unimplemented!() }
        fn chain_id(&self) -> u64 { unimplemented!() }
        fn transact_raw(&mut self, _: Self::Tx) -> core::result::Result<revm::context::result::ExecResultAndState<revm::context::result::ExecutionResult<Self::HaltReason>>, Self::Error> { unimplemented!() }
        fn transact_system_call(&mut self, _: alloy_primitives::Address, _: alloy_primitives::Address, _: alloy_primitives::Bytes) -> core::result::Result<revm::context::result::ExecResultAndState<revm::context::result::ExecutionResult<Self::HaltReason>>, Self::Error> { unimplemented!() }
        fn finish(self) -> (Self::DB, reth_evm::EvmEnv<Self::Spec>) { unimplemented!() }
        fn set_inspector_enabled(&mut self, _: bool) {}
        fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) { unimplemented!() }
        fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) { unimplemented!() }
    }


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
            alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false),
        );
        let mut evm = StubEvm::default();
        let ctx = ReceiptBuilderCtx {
            tx: &tx,
            result: revm::context::result::ExecutionResult::Success {
                reason: revm::context::result::SuccessReason::Stop,
                gas_used: 0,
                gas_refunded: 0,
                logs: Vec::<Log>::new(),
                output: revm::context::result::Output::Call(alloy_primitives::Bytes::default()),
            },
            cumulative_gas_used: 0,
            state: &revm::state::EvmState::default(), evm: &mut evm,
        };
        let res = builder.build_receipt::<StubEvm>(ctx);
        assert!(res.is_err());
    }

    #[test]
    fn non_deposit_tx_types_build_legacy_receipts_without_l1_fields() {

        let builder = ArbRethReceiptBuilder::default();

        fn run_tx(
            builder: &ArbRethReceiptBuilder,
            tx: &reth_arbitrum_primitives::ArbTransactionSigned,
        ) -> ArbReceipt {
            let base_result = revm::context::result::ExecutionResult::Success {
                reason: revm::context::result::SuccessReason::Stop,
                gas_used: 0,
                gas_refunded: 0,
                logs: Vec::<Log>::new(),
                output: revm::context::result::Output::Call(alloy_primitives::Bytes::default()),
            };
            let mut evm = StubEvm::default();
            let ctx = ReceiptBuilderCtx {
                tx,
                result: base_result.clone(),
                cumulative_gas_used: 21000,
                state: &revm::state::EvmState::default(), evm: &mut evm,
            };
            builder.build_receipt::<StubEvm>(ctx).expect("non-deposit should build")
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
            data: Vec::new().into(),
        };
        let tx_unsigned = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Unsigned(unsigned), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let r = run_tx(&builder, &tx_unsigned);
        match r { ArbReceipt::Legacy(rec) => {
            match rec.status { Eip658Value::Eip658(true) => {}, _ => panic!("expected EIP-658 status") }
        }, _ => panic!("expected Legacy receipt") }

        let contract = ArbContractTx {
            chain_id: U256::from(42161u64),
            request_id: b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            from: address!("0000000000000000000000000000000000000002"),
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000002")),
            value: U256::ZERO,
            data: Vec::new().into(),
        };
        let tx_contract = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Contract(contract), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let _ = run_tx(&builder, &tx_contract);

        let retry = ArbRetryTx {
            chain_id: U256::from(42161u64),
            from: address!("0000000000000000000000000000000000000003"),
            nonce: 3,
            gas_fee_cap: U256::from(1000u64),
            gas: 21000,
            to: Some(address!("0000000000000000000000000000000000000004")),
            value: U256::ZERO,
            data: Vec::new().into(),
            ticket_id: b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            refund_to: address!("0000000000000000000000000000000000000005"),
            max_refund: U256::ZERO,
            submission_fee_refund: U256::ZERO,
        };
        let tx_retry = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Retry(retry), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let _ = run_tx(&builder, &tx_retry);

        let srt = ArbSubmitRetryableTx {
            chain_id: U256::from(42161u64),
            request_id: b256!("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
            from: address!("0000000000000000000000000000000000000006"),
            l1_base_fee: U256::from(1u64),
            deposit_value: U256::from(2u64),
            gas_fee_cap: U256::from(4u64),
            gas: 21000,
            retry_to: address!("0000000000000000000000000000000000000008").into(),
            retry_value: U256::from(3u64),
            beneficiary: address!("0000000000000000000000000000000000000007"),
            max_submission_fee: U256::from(5u64),
            fee_refund_addr: address!("0000000000000000000000000000000000000006"),
            retry_data: Vec::new().into(),
        };
        let tx_srt = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::SubmitRetryable(srt), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let _ = run_tx(&builder, &tx_srt);

        let itx = ArbInternalTx {
            chain_id: U256::from(42161u64),
            data: Vec::new().into(),
        };
        let tx_itx = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Internal(itx), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let _ = run_tx(&builder, &tx_itx);

        let leg = alloy_consensus::TxLegacy {
            chain_id: Some(42161),
            nonce: 0,
            gas_price: 1u128,
            gas_limit: 21000,
            to: Some(address!("000000000000000000000000000000000000000b")).into(),
            value: U256::from(0u64),
            input: Vec::new().into(),
        };
        let tx_legacy = reth_arbitrum_primitives::ArbTransactionSigned::new_unhashed(
            ArbTypedTransaction::Legacy(leg), alloy_primitives::Signature::new(U256::ZERO, U256::ZERO, false));
        let _ = run_tx(&builder, &tx_legacy);
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
