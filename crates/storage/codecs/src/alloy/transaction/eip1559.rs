use crate::Compact;
use alloy_consensus::TxEip1559 as AlloyTxEip1559;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Bytes, ChainId, TxKind, U256};
/// [EIP-1559 Transaction](https://eips.ethereum.org/EIPS/eip-1559)
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::TxEip1559`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Compact, Default)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize))]
#[cfg_attr(test, crate::add_arbitrary_tests(compact))]
pub(crate) struct TxEip1559 {
    chain_id: ChainId,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    to: TxKind,
    value: U256,
    access_list: AccessList,
    input: Bytes,
}

impl Compact for AlloyTxEip1559 {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxEip1559 {
            chain_id: self.chain_id,
            nonce: self.nonce,
            gas_limit: self.gas_limit,
            max_fee_per_gas: self.max_fee_per_gas,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            to: self.to,
            value: self.value,
            access_list: self.access_list.clone(),
            input: self.input.clone(),
        };

        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, _) = TxEip1559::from_compact(buf, len);

        let alloy_tx = Self {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            gas_limit: tx.gas_limit,
            max_fee_per_gas: tx.max_fee_per_gas,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
            to: tx.to,
            value: tx.value,
            access_list: tx.access_list,
            input: tx.input,
        };

        (alloy_tx, buf)
    }
}
