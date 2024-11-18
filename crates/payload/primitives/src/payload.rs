use crate::{MessageValidationKind, PayloadAttributes};
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::B256;
use alloy_rpc_types_engine::ExecutionPayload;

/// Either an [`ExecutionPayload`] or a types that implements the [`PayloadAttributes`] trait.
///
/// This is a helper type to unify pre-validation of version specific fields of the engine API.
#[derive(Debug)]
pub enum PayloadOrAttributes<'a, Attributes> {
    /// An [`ExecutionPayload`] and optional parent beacon block root.
    ExecutionPayload {
        /// The inner execution payload
        payload: &'a ExecutionPayload,
        /// The parent beacon block root
        parent_beacon_block_root: Option<B256>,
    },
    /// A payload attributes type.
    PayloadAttributes(&'a Attributes),
}

impl<'a, Attributes> PayloadOrAttributes<'a, Attributes> {
    /// Construct a [`PayloadOrAttributes`] from an [`ExecutionPayload`] and optional parent beacon
    /// block root.
    pub const fn from_execution_payload(
        payload: &'a ExecutionPayload,
        parent_beacon_block_root: Option<B256>,
    ) -> Self {
        Self::ExecutionPayload { payload, parent_beacon_block_root }
    }

    /// Construct a [`PayloadOrAttributes::PayloadAttributes`] variant
    pub const fn from_attributes(attributes: &'a Attributes) -> Self {
        Self::PayloadAttributes(attributes)
    }
}

impl<Attributes> PayloadOrAttributes<'_, Attributes>
where
    Attributes: PayloadAttributes,
{
    /// Return the withdrawals for the payload or attributes.
    pub fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        match self {
            Self::ExecutionPayload { payload, .. } => payload.withdrawals(),
            Self::PayloadAttributes(attributes) => attributes.withdrawals(),
        }
    }

    /// Return the timestamp for the payload or attributes.
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::ExecutionPayload { payload, .. } => payload.timestamp(),
            Self::PayloadAttributes(attributes) => attributes.timestamp(),
        }
    }

    /// Return the parent beacon block root for the payload or attributes.
    pub fn parent_beacon_block_root(&self) -> Option<B256> {
        match self {
            Self::ExecutionPayload { parent_beacon_block_root, .. } => *parent_beacon_block_root,
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

impl<'a, AttributesType> From<&'a AttributesType> for PayloadOrAttributes<'a, AttributesType>
where
    AttributesType: PayloadAttributes,
{
    fn from(attributes: &'a AttributesType) -> Self {
        Self::PayloadAttributes(attributes)
    }
}
