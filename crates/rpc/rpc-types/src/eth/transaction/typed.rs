#![allow(missing_docs)]
//! The [`TransactionRequest`][crate::TransactionRequest] is a universal representation for a
//! transaction deserialized from the json input of an RPC call. Depending on what fields are set,
//! it can be converted into the container type [`TypedTransactionRequest`].

use reth_primitives::{
    AccessList, Address, Bytes, Transaction, TxEip1559, TxEip2930, TxLegacy, U128, U256,
};
use reth_rlp::{BufMut, Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable};
use serde::{Deserialize, Serialize};

/// Container type for various Ethereum transaction requests
///
/// Its variants correspond to specific allowed transactions:
/// 1. Legacy (pre-EIP2718) [`LegacyTransactionRequest`]
/// 2. EIP2930 (state access lists) [`EIP2930TransactionRequest`]
/// 3. EIP1559 [`EIP1559TransactionRequest`]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedTransactionRequest {
    Legacy(LegacyTransactionRequest),
    EIP2930(EIP2930TransactionRequest),
    EIP1559(EIP1559TransactionRequest),
}

impl TypedTransactionRequest {
    /// coverts a typed transaction request into a primitive transaction
    pub fn into_transaction(self) -> Transaction {
        match self {
            TypedTransactionRequest::Legacy(tx) => Transaction::Legacy(TxLegacy {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                gas_price: u128::from_be_bytes(tx.gas_price.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
            }),
            TypedTransactionRequest::EIP2930(tx) => Transaction::Eip2930(TxEip2930 {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                gas_price: u128::from_be_bytes(tx.gas_price.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
                access_list: tx.access_list,
            }),
            TypedTransactionRequest::EIP1559(tx) => Transaction::Eip1559(TxEip1559 {
                chain_id: tx.chain_id,
                nonce: u64::from_be_bytes(tx.nonce.to_be_bytes()),
                max_fee_per_gas: u128::from_be_bytes(tx.max_fee_per_gas.to_be_bytes()),
                gas_limit: u64::from_be_bytes(tx.gas_limit.to_be_bytes()),
                to: tx.kind.into(),
                value: u128::from_be_bytes(tx.value.to_be_bytes()),
                input: tx.input,
                access_list: tx.access_list,
                max_priority_fee_per_gas: u128::from_be_bytes(
                    tx.max_priority_fee_per_gas.to_be_bytes(),
                ),
            }),
        }
    }
}

/// Represents a legacy transaction request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyTransactionRequest {
    pub nonce: U256,
    pub gas_price: U128,
    pub gas_limit: U256,
    pub kind: TransactionKind,
    pub value: U256,
    pub input: Bytes,
    pub chain_id: Option<u64>,
}

/// Represents an EIP-2930 transaction request
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct EIP2930TransactionRequest {
    pub chain_id: u64,
    pub nonce: U256,
    pub gas_price: U128,
    pub gas_limit: U256,
    pub kind: TransactionKind,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
}

/// Represents an EIP-1559 transaction request
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct EIP1559TransactionRequest {
    pub chain_id: u64,
    pub nonce: U256,
    pub max_priority_fee_per_gas: U128,
    pub max_fee_per_gas: U128,
    pub gas_limit: U256,
    pub kind: TransactionKind,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
}

/// Represents the `to` field of a transaction request
///
/// This determines what kind of transaction this is
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TransactionKind {
    /// Transaction will call this address or transfer funds to this address
    Call(Address),
    /// No `to` field set, this transaction will create a contract
    Create,
}

// == impl TransactionKind ==

impl TransactionKind {
    /// If this transaction is a call this returns the address of the callee
    pub fn as_call(&self) -> Option<&Address> {
        match self {
            TransactionKind::Call(to) => Some(to),
            TransactionKind::Create => None,
        }
    }
}

impl Encodable for TransactionKind {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            TransactionKind::Call(to) => to.encode(out),
            TransactionKind::Create => ([]).encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            TransactionKind::Call(to) => to.length(),
            TransactionKind::Create => ([]).length(),
        }
    }
}

impl Decodable for TransactionKind {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if let Some(&first) = buf.first() {
            if first == 0x80 {
                *buf = &buf[1..];
                Ok(TransactionKind::Create)
            } else {
                let addr = <Address as Decodable>::decode(buf)?;
                Ok(TransactionKind::Call(addr))
            }
        } else {
            Err(DecodeError::InputTooShort)
        }
    }
}

impl From<TransactionKind> for reth_primitives::TransactionKind {
    fn from(kind: TransactionKind) -> Self {
        match kind {
            TransactionKind::Call(to) => reth_primitives::TransactionKind::Call(to),
            TransactionKind::Create => reth_primitives::TransactionKind::Create,
        }
    }
}
