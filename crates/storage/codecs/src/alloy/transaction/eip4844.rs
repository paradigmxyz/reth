use crate::{Compact, CompactPlaceholder};
use alloy_consensus::transaction::TxEip4844 as AlloyTxEip4844;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Address, Bytes, ChainId, B256, U256};
use reth_codecs_derive::add_arbitrary_tests;
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// [EIP-4844 Blob Transaction](https://eips.ethereum.org/EIPS/eip-4844#blob-transaction)
///
/// This is a helper type to use derive on it instead of manually managing `bitfield`.
///
/// By deriving `Compact` here, any future changes or enhancements to the `Compact` derive
/// will automatically apply to this type.
///
/// Notice: Make sure this struct is 1:1 with [`alloy_consensus::transaction::TxEip4844`]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
pub(crate) struct TxEip4844 {
    chain_id: ChainId,
    nonce: u64,
    gas_limit: u64,
    max_fee_per_gas: u128,
    max_priority_fee_per_gas: u128,
    placeholder: Option<CompactPlaceholder>,
    to: Address,
    value: U256,
    access_list: AccessList,
    blob_versioned_hashes: Vec<B256>,
    max_fee_per_blob_gas: u128,
    input: Bytes,
}

impl Compact for AlloyTxEip4844 {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let tx = TxEip4844 {
            chain_id: self.chain_id,
            nonce: self.nonce,
            gas_limit: self.gas_limit as u64,
            max_fee_per_gas: self.max_fee_per_gas,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            placeholder: Some(()),
            to: self.to,
            value: self.value,
            access_list: self.access_list.clone(),
            blob_versioned_hashes: self.blob_versioned_hashes.clone(),
            max_fee_per_blob_gas: self.max_fee_per_blob_gas,
            input: self.input.clone(),
        };
        tx.to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (tx, _) = TxEip4844::from_compact(buf, len);
        let alloy_tx = Self {
            chain_id: tx.chain_id,
            nonce: tx.nonce,
            gas_limit: tx.gas_limit as u128,
            max_fee_per_gas: tx.max_fee_per_gas,
            max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
            to: tx.to,
            value: tx.value,
            access_list: tx.access_list,
            blob_versioned_hashes: tx.blob_versioned_hashes,
            max_fee_per_blob_gas: tx.max_fee_per_blob_gas,
            input: tx.input,
        };
        (alloy_tx, buf)
    }
}
