use crate::Compact;
use alloy_consensus::TxEip2930 as AlloyTxEip2930;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Bytes, ChainId, TxKind, U256};
use reth_codecs_derive::add_arbitrary_tests;

/// Transaction with an [`AccessList`] ([EIP-2930](https://eips.ethereum.org/EIPS/eip-2930)).
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::TxEip2930`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(compact)]
pub(crate) struct TxEip2930 {
    chain_id: ChainId,
    nonce: u64,
    gas_price: u128,
    gas_limit: u64,
    to: TxKind,
    value: U256,
    access_list: AccessList,
    input: Bytes,
}

impl Compact for AlloyTxEip2930 {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxEip2930 {
            chain_id: self.chain_id,
            nonce: self.nonce,
            gas_price: self.gas_price,
            gas_limit: self.gas_limit,
            to: self.to,
            value: self.value,
            access_list: self.access_list.clone(),
            input: self.input.clone(),
        };
        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, _) = TxEip2930::from_compact(buf, len);
        let alloy_tx = Self {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            gas_price: tx.gas_price,
            gas_limit: tx.gas_limit,
            to: tx.to,
            value: tx.value,
            access_list: tx.access_list,
            input: tx.input,
        };
        (alloy_tx, buf)
    }
}
