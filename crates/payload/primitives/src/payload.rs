use crate::{MessageValidationKind, PayloadAttributes};
use alloc::vec::Vec;
use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::B256;
use alloy_rpc_types_engine::ExecutionData;
use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};

/// An execution payload.
pub trait ExecutionPayload:
    Serialize + DeserializeOwned + Debug + Clone + Send + Sync + 'static
{
    /// Returns the parent hash of the block.
    fn parent_hash(&self) -> B256;

    /// Returns the hash of the block.
    fn block_hash(&self) -> B256;

    /// Returns the number of the block.
    fn block_number(&self) -> u64;

    /// Returns the withdrawals for the payload, if it exists.
    fn withdrawals(&self) -> Option<&Vec<Withdrawal>>;

    /// Return the parent beacon block root for the payload, if it exists.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Returns the timestamp to be used in the payload.
    fn timestamp(&self) -> u64;

    /// Gas used by the payload
    fn gas_used(&self) -> u64;
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

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.payload.withdrawals()
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
}

/// Either a type that implements the [`ExecutionPayload`] or a type that implements the
/// [`PayloadAttributes`] trait.
///
/// This is a helper type to unify pre-validation of version specific fields of the engine API.
#[derive(Debug)]
pub enum PayloadOrAttributes<'a, Payload, Attributes> {
    /// An [`ExecutionPayload`]
    ExecutionPayload(&'a Payload),
    /// A payload attributes type.
    PayloadAttributes(&'a Attributes),
}

impl<'a, Payload, Attributes> PayloadOrAttributes<'a, Payload, Attributes> {
    /// Construct a [`PayloadOrAttributes::ExecutionPayload`] variant
    pub const fn from_execution_payload(payload: &'a Payload) -> Self {
        Self::ExecutionPayload(payload)
    }

    /// Construct a [`PayloadOrAttributes::PayloadAttributes`] variant
    pub const fn from_attributes(attributes: &'a Attributes) -> Self {
        Self::PayloadAttributes(attributes)
    }
}

impl<Payload, Attributes> PayloadOrAttributes<'_, Payload, Attributes>
where
    Payload: ExecutionPayload,
    Attributes: PayloadAttributes,
{
    /// Return the withdrawals for the payload or attributes.
    pub fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        match self {
            Self::ExecutionPayload(payload) => payload.withdrawals(),
            Self::PayloadAttributes(attributes) => attributes.withdrawals(),
        }
    }

    /// Return the timestamp for the payload or attributes.
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::ExecutionPayload(payload) => payload.timestamp(),
            Self::PayloadAttributes(attributes) => attributes.timestamp(),
        }
    }

    /// Return the parent beacon block root for the payload or attributes.
    pub fn parent_beacon_block_root(&self) -> Option<B256> {
        match self {
            Self::ExecutionPayload(payload) => payload.parent_beacon_block_root(),
            Self::PayloadAttributes(attributes) => attributes.parent_beacon_block_root(),
        }
    }

    /// Return a [`MessageValidationKind`] for the payload or attributes.
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

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.payload.as_v2().map(|p| &p.withdrawals)
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
}

/// Special implementation for Ethereum types that provides additional helper methods
impl<Attributes> PayloadOrAttributes<'_, ExecutionData, Attributes>
where
    Attributes: PayloadAttributes,
{
    /// Return the execution requests from the payload, if available.
    ///
    /// This will return `Some(requests)` only if:
    /// - The payload is an `ExecutionData` (not `PayloadAttributes`)
    /// - The payload has Prague payload fields
    /// - The Prague fields contain requests (not a hash)
    ///
    /// Returns `None` in all other cases.
    pub fn execution_requests(&self) -> Option<&Requests> {
        if let Self::ExecutionPayload(payload) = self {
            payload.sidecar.requests()
        } else {
            None
        }
    }
}
