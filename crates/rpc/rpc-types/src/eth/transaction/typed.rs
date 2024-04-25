//! The [`TransactionRequest`][crate::TransactionRequest] is a universal representation for a
//! transaction deserialized from the json input of an RPC call. Depending on what fields are set,
//! it can be converted into the container type [`TypedTransactionRequest`].

use alloy_primitives::{Bytes, TxKind, B256, U256};
use alloy_rpc_types::{AccessList, BlobTransactionSidecar};

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
    pub nonce: u64,
    /// The gas price for the transaction
    pub gas_price: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TxKind,
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
    pub nonce: u64,
    /// The gas price for the transaction
    pub gas_price: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TxKind,
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
    pub nonce: u64,
    /// The maximum priority fee per gas for the transaction
    pub max_priority_fee_per_gas: U256,
    /// The maximum fee per gas for the transaction
    pub max_fee_per_gas: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TxKind,
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
    pub nonce: u64,
    /// The maximum priority fee per gas for the transaction
    pub max_priority_fee_per_gas: U256,
    /// The maximum fee per gas for the transaction
    pub max_fee_per_gas: U256,
    /// The gas limit for the transaction
    pub gas_limit: U256,
    /// The kind of transaction (e.g., Call, Create)
    pub kind: TxKind,
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
