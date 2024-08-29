use crate::Compact;
use alloy_consensus::transaction::TxEip7702 as AlloyTxEip7702;
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use alloy_primitives::{Bytes, ChainId, TxKind, U256};
use reth_codecs_derive::add_arbitrary_tests;
use serde::{Deserialize, Serialize};

/// [EIP-7702 Set Code Transaction](https://eips.ethereum.org/EIPS/eip-7702)
///
/// Set EOA account code for one transaction
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize, Compact)]
#[cfg_attr(test, derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(compact)]
struct TxEip7702 {
    /// Added as EIP-155: Simple replay attack protection
    pub chain_id: ChainId,
    /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
    pub nonce: u64,
    /// A scalar value equal to the number of
    /// Wei to be paid per unit of gas for all computation
    /// costs incurred as a result of the execution of this transaction; formally Tp.
    ///
    /// As ethereum circulation is around 120mil eth as of 2022 that is around
    /// 120000000000000000000000000 wei we are safe to use u128 as its max number is:
    /// 340282366920938463463374607431768211455
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
    /// The 160-bit address of the message call’s recipient or, for a contract creation
    /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
    pub to: TxKind,
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
    /// Authorizations are used to temporarily set the code of its signer to
    /// the code referenced by `address`. These also include a `chain_id` (which
    /// can be set to zero and not evaluated) as well as an optional `nonce`.
    pub authorization_list: Vec<SignedAuthorization>,
    /// Input has two uses depending if the transaction `to` field is [`TxKind::Create`] or
    /// [`TxKind::Call`].
    ///
    /// Input as init code, or if `to` is [`TxKind::Create`]: An unlimited size byte array
    /// specifying the EVM-code for the account initialisation procedure `CREATE`
    ///
    /// Input as data, or if `to` is [`TxKind::Call`]: An unlimited size byte array specifying the
    /// input data of the message call, formally Td.
    pub input: Bytes,
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
            gas_limit: self.gas_limit as u64,
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
            gas_limit: tx.gas_limit as u128,
            to: tx.to,
            value: tx.value,
            input: tx.input,
            access_list: tx.access_list,
            authorization_list: tx.authorization_list,
        };
        (alloy_tx, buf)
    }
}
