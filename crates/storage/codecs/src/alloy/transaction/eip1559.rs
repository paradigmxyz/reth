use crate::Compact;
use alloy_consensus::TxEip1559 as AlloyTxEip1559;
use alloy_eips::eip2930::AccessList;
use alloy_primitives::{Bytes, ChainId, TxKind, U256};
use reth_codecs_derive::reth_codec;
use serde::{Deserialize, Serialize};

/// A transaction with a priority fee ([EIP-1559](https://eips.ethereum.org/EIPS/eip-1559)).
#[reth_codec]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub(crate) struct TxEip1559 {
    /// Added as EIP-155: Simple replay attack protection
    chain_id: ChainId,

    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    nonce: u64,

    /// A scalar value equal to the maximum
    /// amount of gas that should be used in executing
    /// this transaction. This is paid up-front, before any
    /// computation is done and may not be increased
    /// later; formally Tg.
    gas_limit: u64,

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
    max_fee_per_gas: u128,

    /// Max Priority fee that transaction is paying
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
    ///
    /// This is also known as `GasTipCap`
    max_priority_fee_per_gas: u128,

    /// The 160-bit address of the message call’s recipient or, for a contract creation
    /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
    to: TxKind,

    /// A scalar value equal to the number of Wei to
    /// be transferred to the message call’s recipient or,
    /// in the case of contract creation, as an endowment
    /// to the newly created account; formally Tv.
    value: U256,

    /// The accessList specifies a list of addresses and storage keys;
    /// these addresses and storage keys are added into the `accessed_addresses`
    /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
    /// A gas cost is charged, though at a discount relative to the cost of
    /// accessing outside the list.
    access_list: AccessList,

    /// Input has two uses depending if the transaction `to` field is [`TxKind::Create`] or
    /// [`TxKind::Call`].
    ///
    /// Input as init code, or if `to` is [`TxKind::Create`]: An unlimited size byte array
    /// specifying the EVM-code for the account initialisation procedure `CREATE`
    ///
    /// Input as data, or if `to` is [`TxKind::Call`]: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
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
            gas_limit: self.gas_limit as u64,
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
            gas_limit: tx.gas_limit.into(),
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
