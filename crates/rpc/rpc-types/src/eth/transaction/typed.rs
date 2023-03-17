#![allow(missing_docs)]
//! The [`TransactionRequest`][crate::TransactionRequest] is a universal representation for a
//! transaction deserialized from the json input of an RPC call. Depending on what fields are set,
//! it can be converted into the container type [`TypedTransactionRequest`].

use reth_primitives::{AccessList, Address, Bytes, U128, U256};
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
