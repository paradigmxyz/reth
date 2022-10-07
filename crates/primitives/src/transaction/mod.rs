mod access_list;
mod signature;
mod tx_type;

use crate::{Address, Bytes, TxHash, U256};
pub use access_list::{AccessList, AccessListItem};
use signature::Signature;
use std::ops::Deref;
pub use tx_type::TxType;

/// Raw Transaction.
/// Transaction type is introduced in EIP-2718: https://eips.ethereum.org/EIPS/eip-2718
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transaction {
    /// Legacy transaciton.
    Legacy {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: Option<u64>,
        /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
        nonce: u64,
        /// A scalar value equal to the number of
        /// Wei to be paid per unit of gas for all computation
        /// costs incurred as a result of the execution of this transaction; formally Tp.
        gas_price: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        gas_limit: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: Option<Address>,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
    },
    /// Transaction with AccessList. https://eips.ethereum.org/EIPS/eip-2930
    Eip2930 {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: u64,
        /// A scalar value equal to the number of transactions sent by the sender; formally Tn.
        nonce: u64,
        /// A scalar value equal to the number of
        /// Wei to be paid per unit of gas for all computation
        /// costs incurred as a result of the execution of this transaction; formally Tp.
        gas_price: u64,
        /// A scalar value equal to the maximum
        /// amount of gas that should be used in executing
        /// this transaction. This is paid up-front, before any
        /// computation is done and may not be increased
        /// later; formally Tg.
        gas_limit: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: Option<Address>,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
        /// The accessList specifies a list of addresses and storage keys;
        /// these addresses and storage keys are added into the `accessed_addresses`
        /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
        /// A gas cost is charged, though at a discount relative to the cost of
        /// accessing outside the list.
        access_list: AccessList,
    },
    /// Transaction with priority fee. https://eips.ethereum.org/EIPS/eip-1559
    Eip1559 {
        /// Added as EIP-155: Simple replay attack protection
        chain_id: u64,
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
        max_fee_per_gas: u64,
        /// Max Priority fee that transaction is paying
        max_priority_fee_per_gas: u64,
        /// The 160-bit address of the message call’s recipient or, for a contract creation
        /// transaction, ∅, used here to denote the only member of B0 ; formally Tt.
        to: Option<Address>,
        /// A scalar value equal to the number of Wei to
        /// be transferred to the message call’s recipient or,
        /// in the case of contract creation, as an endowment
        /// to the newly created account; formally Tv.
        value: U256,
        /// Input has two uses depending if transaction is Create or Call (if `to` field is None or
        /// Some). init: An unlimited size byte array specifying the
        /// EVM-code for the account initialisation procedure CREATE,
        /// data: An unlimited size byte array specifying the
        /// input data of the message call, formally Td.
        input: Bytes,
        /// The accessList specifies a list of addresses and storage keys;
        /// these addresses and storage keys are added into the `accessed_addresses`
        /// and `accessed_storage_keys` global sets (introduced in EIP-2929).
        /// A gas cost is charged, though at a discount relative to the cost of
        /// accessing outside the list.
        access_list: AccessList,
    },
}

impl Transaction {
    /// Heavy operation that return hash over rlp encoded transaction.
    /// It is only used for signature signing.
    pub fn signature_hash(&self) -> TxHash {
        todo!()
    }
}

/// Signed transaction.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionSigned {
    transaction: Transaction,
    hash: TxHash,
    signature: Signature,
}

impl AsRef<Transaction> for TransactionSigned {
    fn as_ref(&self) -> &Transaction {
        &self.transaction
    }
}

impl Deref for TransactionSigned {
    type Target = Transaction;

    fn deref(&self) -> &Self::Target {
        &self.transaction
    }
}

impl TransactionSigned {
    /// Transaction signature.
    pub fn signature(&self) -> &Signature {
        &self.signature
    }

    /// Transaction hash. Used to identify transaction.
    pub fn hash(&self) -> TxHash {
        self.hash
    }
}
