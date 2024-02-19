//! The [`TransactionRequest`][crate::TransactionRequest] is a universal representation for a
//! transaction deserialized from the json input of an RPC call. Depending on what fields are set,
//! it can be converted into the container type [`TypedTransactionRequest`].

use alloy_primitives::{Address, Bytes, B256, U256, U64};
use alloy_rlp::{BufMut, Decodable, Encodable, Error as RlpError};
use alloy_rpc_types::{AccessList, BlobTransactionSidecar};
use serde::{Deserialize, Serialize};

/// Container type for various Ethereum transaction requests
///
/// Its variants correspond to specific allowed transactions:
/// 1. Legacy (pre-EIP2718) [`LegacyTransactionRequest`]
/// 2. EIP2930 (state access lists) [`EIP2930TransactionRequest`]
/// 3. EIP1559 [`EIP1559TransactionRequest`]
/// 4. EIP4844 [`EIP4844TransactionRequest`]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TypedTransactionRequest {
    /// Represents a Legacy (pre-EIP2718) transaction request
    Legacy(LegacyTransactionRequest),
    /// Represents an EIP2930 (state access lists) transaction request
    EIP2930(EIP2930TransactionRequest),
    /// Represents an EIP1559 transaction request
    EIP1559(EIP1559TransactionRequest),
    /// Represents an EIP4844 transaction request
    EIP4844(EIP4844TransactionRequest),
}

/// Represents a legacy transaction request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyTransactionRequest {
    /// The nonce of the transaction
    pub nonce: U64,
    /// The gas price for the transaction
    pub gas_price: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TransactionKind,
    /// The value of the transaction
    pub value: U256,
    /// The input data for the transaction
    pub input: Bytes,
    /// The optional chain ID for the transaction
    pub chain_id: Option<u64>,
}

/// Represents an EIP-2930 transaction request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EIP2930TransactionRequest {
    /// The chain ID for the transaction
    pub chain_id: u64,
    /// The nonce of the transaction
    pub nonce: U64,
    /// The gas price for the transaction
    pub gas_price: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TransactionKind,
    /// The value of the transaction
    pub value: U256,
    /// The input data for the transaction
    pub input: Bytes,
    /// The access list for the transaction
    pub access_list: AccessList,
}

/// Represents an EIP-1559 transaction request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EIP1559TransactionRequest {
    /// The chain ID for the transaction
    pub chain_id: u64,
    /// The nonce of the transaction
    pub nonce: U64,
    /// The maximum priority fee per gas for the transaction
    pub max_priority_fee_per_gas: U256,
    /// The maximum fee per gas for the transaction
    pub max_fee_per_gas: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TransactionKind,
    /// The value of the transaction
    pub value: U256,
    /// The input data for the transaction
    pub input: Bytes,
    /// The access list for the transaction
    pub access_list: AccessList,
}

/// Represents an EIP-4844 transaction request
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EIP4844TransactionRequest {
    /// The chain ID for the transaction
    pub chain_id: u64,
    /// The nonce of the transaction
    pub nonce: U64,
    /// The maximum priority fee per gas for the transaction
    pub max_priority_fee_per_gas: U256,
    /// The maximum fee per gas for the transaction
    pub max_fee_per_gas: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TransactionKind,
    /// The value of the transaction
    pub value: U256,
    /// The input data for the transaction
    pub input: Bytes,
    /// The access list for the transaction
    pub access_list: AccessList,
    /// The maximum fee per blob gas for the transaction
    pub max_fee_per_blob_gas: U256,
    /// Versioned hashes associated with the transaction
    pub blob_versioned_hashes: Vec<B256>,
    /// Sidecar information for the transaction
    pub sidecar: BlobTransactionSidecar,
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
    /// This encodes the `to` field of a transaction request.
    /// If the [TransactionKind] is a [TransactionKind::Call] it will encode the inner address:
    /// `rlp(address)`
    ///
    /// If the [TransactionKind] is a [TransactionKind::Create] it will encode an empty list:
    /// `rlp([])`, which is also
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
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        if let Some(&first) = buf.first() {
            if first == 0x80 {
                *buf = &buf[1..];
                Ok(TransactionKind::Create)
            } else {
                let addr = <Address as Decodable>::decode(buf)?;
                Ok(TransactionKind::Call(addr))
            }
        } else {
            Err(RlpError::InputTooShort)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_kind_encoding_sanity() {
        // check the 0x80 encoding for Create
        let mut buf = Vec::new();
        TransactionKind::Create.encode(&mut buf);
        assert_eq!(buf, vec![0x80]);

        // check decoding
        let buf = [0x80];
        let decoded = TransactionKind::decode(&mut &buf[..]).unwrap();
        assert_eq!(decoded, TransactionKind::Create);
    }
}
