//! Types and traits for execution payload data structures.

use crate::{MessageValidationKind, PayloadAttributes};
use alloc::vec::Vec;
use alloy_eips::{
    eip1898::BlockWithParent,
    eip4895::{Withdrawal, Withdrawals},
    eip7685::Requests,
    BlockNumHash,
};
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::ExecutionData;
use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};

/// Represents the core data structure of an execution payload.
///
/// Contains all necessary information to execute and validate a block, including
/// headers, transactions, and consensus fields. Provides a unified interface
/// regardless of protocol version.
pub trait ExecutionPayload:
    Serialize + DeserializeOwned + Debug + Clone + Send + Sync + 'static
{
    /// Returns the hash of this block's parent.
    fn parent_hash(&self) -> B256;

    /// Returns this block's hash.
    fn block_hash(&self) -> B256;

    /// Returns this block's number (height).
    fn block_number(&self) -> u64;

    /// Returns this block's number hash.
    fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.block_number(), self.block_hash())
    }

    /// Returns a [`BlockWithParent`] for this block.
    fn block_with_parent(&self) -> BlockWithParent {
        BlockWithParent::new(self.parent_hash(), self.num_hash())
    }

    /// Returns the withdrawals included in this payload.
    ///
    /// Returns `None` for pre-Shanghai blocks.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Returns the access list included in this payload.
    ///
    /// Returns `None` for pre-Amsterdam blocks.
    fn block_access_list(&self) -> Option<&Bytes>;

    /// Returns the beacon block root associated with this payload.
    ///
    /// Returns `None` for pre-merge payloads.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Returns this block's timestamp (seconds since Unix epoch).
    fn timestamp(&self) -> u64;

    /// Returns the total gas consumed by all transactions in this block.
    fn gas_used(&self) -> u64;

    /// Returns the number of transactions in the payload.
    fn transaction_count(&self) -> usize;
}

impl ExecutionPayload for ExecutionData {
    fn parent_hash(&self) -> B256 {
        self.payload.parent_hash()
    }

    fn block_hash(&self) -> B256 {
        self.payload.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.payload.block_number()
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.payload.withdrawals().map(|w| {
            // SAFETY: `Withdrawals` is a single-field newtype wrapping `Vec<Withdrawal>`
            // (`Withdrawals(pub Vec<Withdrawal>)`). Because it has exactly one field and no
            // explicit `repr`, the compiler lays it out identically to `Vec<Withdrawal>`, so
            // transmuting a shared reference is sound. This is analogous to the well-known
            // pattern for `repr(transparent)` newtypes and avoids an unnecessary heap clone.
            unsafe { &*(w as *const Vec<Withdrawal> as *const Withdrawals) }
        })
    }

    fn block_access_list(&self) -> Option<&Bytes> {
        None
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.sidecar.parent_beacon_block_root()
    }

    fn timestamp(&self) -> u64 {
        self.payload.timestamp()
    }

    fn gas_used(&self) -> u64 {
        self.payload.as_v1().gas_used
    }

    fn transaction_count(&self) -> usize {
        self.payload.as_v1().transactions.len()
    }
}

/// A unified type for handling both execution payloads and payload attributes.
///
/// Enables generic validation and processing logic for both complete payloads
/// and payload attributes, useful for version-specific validation.
#[derive(Debug)]
pub enum PayloadOrAttributes<'a, Payload, Attributes> {
    /// A complete execution payload containing block data
    ExecutionPayload(&'a Payload),
    /// Attributes specifying how to build a new payload
    PayloadAttributes(&'a Attributes),
}

impl<'a, Payload, Attributes> PayloadOrAttributes<'a, Payload, Attributes> {
    /// Creates a `PayloadOrAttributes` from an execution payload reference
    pub const fn from_execution_payload(payload: &'a Payload) -> Self {
        Self::ExecutionPayload(payload)
    }

    /// Creates a `PayloadOrAttributes` from a payload attributes reference
    pub const fn from_attributes(attributes: &'a Attributes) -> Self {
        Self::PayloadAttributes(attributes)
    }
}

impl<Payload, Attributes> PayloadOrAttributes<'_, Payload, Attributes>
where
    Payload: ExecutionPayload,
    Attributes: PayloadAttributes,
{
    /// Returns withdrawals from either the payload or attributes.
    pub fn withdrawals(&self) -> Option<&Withdrawals> {
        match self {
            Self::ExecutionPayload(payload) => payload.withdrawals(),
            Self::PayloadAttributes(attributes) => attributes.withdrawals(),
        }
    }

    /// Returns the timestamp from either the payload or attributes.
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::ExecutionPayload(payload) => payload.timestamp(),
            Self::PayloadAttributes(attributes) => attributes.timestamp(),
        }
    }

    /// Returns the parent beacon block root from either the payload or attributes.
    pub fn parent_beacon_block_root(&self) -> Option<B256> {
        match self {
            Self::ExecutionPayload(payload) => payload.parent_beacon_block_root(),
            Self::PayloadAttributes(attributes) => attributes.parent_beacon_block_root(),
        }
    }

    /// Determines the validation context based on the contained type.
    pub const fn message_validation_kind(&self) -> MessageValidationKind {
        match self {
            Self::ExecutionPayload { .. } => MessageValidationKind::Payload,
            Self::PayloadAttributes(_) => MessageValidationKind::PayloadAttributes,
        }
    }
}

impl<'a, Payload, AttributesType> From<&'a AttributesType>
    for PayloadOrAttributes<'a, Payload, AttributesType>
where
    AttributesType: PayloadAttributes,
{
    fn from(attributes: &'a AttributesType) -> Self {
        Self::PayloadAttributes(attributes)
    }
}

#[cfg(feature = "op")]
impl ExecutionPayload for op_alloy_rpc_types_engine::OpExecutionData {
    fn parent_hash(&self) -> B256 {
        self.parent_hash()
    }

    fn block_hash(&self) -> B256 {
        self.block_hash()
    }

    fn block_number(&self) -> u64 {
        self.block_number()
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.payload.as_v2().map(|p| {
            // SAFETY: `Withdrawals` is a single-field newtype wrapping `Vec<Withdrawal>`
            // (`Withdrawals(pub Vec<Withdrawal>)`). Because it has exactly one field and no
            // explicit `repr`, the compiler lays it out identically to `Vec<Withdrawal>`, so
            // transmuting a shared reference is sound. This is analogous to the well-known
            // pattern for `repr(transparent)` newtypes and avoids an unnecessary heap clone.
            unsafe { &*(&raw const p.withdrawals as *const Withdrawals) }
        })
    }

    fn block_access_list(&self) -> Option<&Bytes> {
        None
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.sidecar.parent_beacon_block_root()
    }

    fn timestamp(&self) -> u64 {
        self.payload.as_v1().timestamp
    }

    fn gas_used(&self) -> u64 {
        self.payload.as_v1().gas_used
    }

    fn transaction_count(&self) -> usize {
        self.payload.as_v1().transactions.len()
    }
}

/// Extended functionality for Ethereum execution payloads
impl<Attributes> PayloadOrAttributes<'_, ExecutionData, Attributes>
where
    Attributes: PayloadAttributes,
{
    /// Extracts execution layer requests from the payload.
    ///
    /// Returns `Some(requests)` if this is an execution payload with request data,
    /// `None` otherwise.
    pub fn execution_requests(&self) -> Option<&Requests> {
        if let Self::ExecutionPayload(payload) = self {
            payload.sidecar.requests()
        } else {
            None
        }
    }
}
