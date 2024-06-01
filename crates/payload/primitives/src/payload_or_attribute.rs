use crate::{
    error::{EngineObjectValidationError, VersionSpecificValidationError},
    PayloadAttributes,
};
use reth_primitives::B256;
use reth_rpc_types::engine::ExecutionPayload;

/// A type that represents whether or not we are validating a payload or payload attributes.
///
/// This is used to ensure that the correct error code is returned when validating the payload or
/// payload attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidationKind {
    /// We are validating fields of a payload attributes.
    PayloadAttributes,
    /// We are validating fields of a payload.
    Payload,
}

impl MessageValidationKind {
    /// Returns an `EngineObjectValidationError` based on the given
    /// `VersionSpecificValidationError` and the current validation kind.
    pub fn to_error(self, error: VersionSpecificValidationError) -> EngineObjectValidationError {
        match self {
            Self::Payload => EngineObjectValidationError::Payload(error),
            Self::PayloadAttributes => EngineObjectValidationError::PayloadAttributes(error),
        }
    }
}

/// Either an [ExecutionPayload] or a types that implements the [PayloadAttributes] trait.
#[derive(Debug)]
pub enum PayloadOrAttributes<'a, AttributesType> {
    /// An [ExecutionPayload] and optional parent beacon block root.
    ExecutionPayload {
        /// The inner execution payload
        payload: &'a ExecutionPayload,
        /// The parent beacon block root
        parent_beacon_block_root: Option<B256>,
    },
    /// A payload attributes type.
    PayloadAttributes(&'a AttributesType),
}

impl<'a, AttributesType> PayloadOrAttributes<'a, AttributesType>
where
    AttributesType: PayloadAttributes,
{
    /// Construct a [PayloadOrAttributes] from an [ExecutionPayload] and optional parent beacon
    /// block root.
    pub const fn from_execution_payload(
        payload: &'a ExecutionPayload,
        parent_beacon_block_root: Option<B256>,
    ) -> Self {
        Self::ExecutionPayload { payload, parent_beacon_block_root }
    }

    /// Return the withdrawals for the payload or attributes.
    pub fn withdrawals(&self) -> Option<&Vec<reth_rpc_types::Withdrawal>> {
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

    /// Return a [MessageValidationKind] for the payload or attributes.
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
