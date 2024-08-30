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
/// A transaction with blob hashes and max blob fee
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
struct TxEip4844 {
    /// Added as EIP-155: Simple replay attack protection
    pub chain_id: ChainId,

    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub nonce: u64,

    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    pub gas_limit: u64,

    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasFeeCap`
    pub max_fee_per_gas: u128,

    /// Max Priority fee that transaction is paying
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasTipCap`
    pub max_priority_fee_per_gas: u128,

    /// TODO(debt): this should be removed if we break the DB.
    /// Makes sure that the Compact bitflag struct has one bit after the above field:
    /// <https://github.com/paradigmxyz/reth/pull/8291#issuecomment-2117545016>
    pub placeholder: Option<CompactPlaceholder>,

    /// The 160-bit address of the message call’s recipient.
    pub to: Address,

    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    pub value: U256,

    /// The accessList specifies a list of addresses and storage keys;
    /// these addresses and storage keys are added into the `accessed_addresses`
    /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
    /// A gas cost is charged, though at a discount relative to the cost of
    /// accessing outside the list.
    pub access_list: AccessList,

    /// It contains a vector of fixed size hash(32 bytes)
    pub blob_versioned_hashes: Vec<B256>,

    /// Max fee per data gas
    ///
    /// aka BlobFeeCap or blobGasFeeCap
    pub max_fee_per_blob_gas: u128,

    /// Unlike other transaction types, where the `input` field has two uses depending on whether
    /// or not the `to` field is [`Create`](crate::TxKind::Create) or
    /// [`Call`](crate::TxKind::Call), EIP-4844 transactions cannot be
    /// [`Create`](crate::TxKind::Create) transactions.
    ///
    /// This means the `input` field has a single use, as data: An unlimited size byte array
    /// specifying the input data of the message call, formally Td.
    pub input: Bytes,
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
            placeholder: None,
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
