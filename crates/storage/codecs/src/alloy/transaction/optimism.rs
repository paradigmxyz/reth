use crate::Compact;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use op_alloy_consensus::TxDeposit as AlloyTxDeposit;
use reth_codecs_derive::add_arbitrary_tests;

/// Deposit transactions, also known as deposits are initiated on L1, and executed on L2.
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`op_alloy_consensus::TxDeposit`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(compact)]
pub(crate) struct TxDeposit {
    source_hash: B256,
    from: Address,
    to: TxKind,
    mint: Option<u128>,
    value: U256,
    gas_limit: u64,
    is_system_transaction: bool,
    input: Bytes,
}

impl Compact for AlloyTxDeposit {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxDeposit {
            source_hash: self.source_hash,
            from: self.from,
            to: self.to,
            mint: self.mint,
            value: self.value,
            gas_limit: self.gas_limit,
            is_system_transaction: self.is_system_transaction,
            input: self.input.clone(),
        };
        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, _) = TxDeposit::from_compact(buf, len);
        let alloy_tx = Self {
            source_hash: tx.source_hash,
            from: tx.from,
            to: tx.to,
            mint: tx.mint,
            value: tx.value,
            gas_limit: tx.gas_limit,
            is_system_transaction: tx.is_system_transaction,
            input: tx.input,
        };
        (alloy_tx, buf)
    }
}
