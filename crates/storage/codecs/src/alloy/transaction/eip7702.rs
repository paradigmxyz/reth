use crate::Compact;
use alloc::vec::Vec;
use alloy_consensus::TxEip7702 as AlloyTxEip7702;
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Address, Bytes, ChainId, U256};
use reth_codecs_derive::add_arbitrary_tests;

/// [EIP-7702 Set Code Transaction](https://eips.ethereum.org/EIPS/eip-7702)
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::TxEip7702`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary, serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(compact)]
pub(crate) struct TxEip7702 {
    chain_id: ChainId,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    to: Address,
    value: U256,
    access_list: AccessList,
    authorization_list: Vec<SignedAuthorization>,
    input: Bytes,
}

impl Compact for AlloyTxEip7702 {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxEip7702 {
            chain_id: self.chain_id,
            nonce: self.nonce,
            max_fee_per_gas: self.max_fee_per_gas,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            gas_limit: self.gas_limit,
            to: self.to,
            value: self.value,
            input: self.input.clone(),
            access_list: self.access_list.clone(),
            authorization_list: self.authorization_list.clone(),
        };
        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, _) = TxEip7702::from_compact(buf, len);
        let alloy_tx = Self {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            max_fee_per_gas: tx.max_fee_per_gas,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
            gas_limit: tx.gas_limit,
            to: tx.to,
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list,
            authorization_list: tx.authorization_list,
        };
        (alloy_tx, buf)
    }
}
