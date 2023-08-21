use reth_primitives::{Withdrawal, H256};
use reth_rpc_types::engine::{ExecutionPayload, PayloadAttributes};

/// Either an [ExecutionPayload] or a [PayloadAttributes].
pub(crate) enum PayloadOrAttributes<'a> {
    /// An [ExecutionPayload] and optional parent beacon block root.
    ExecutionPayload {
        /// The inner execution payload
        payload: &'a ExecutionPayload,
        /// The parent beacon block root
        parent_beacon_block_root: Option<H256>,
    },
    /// A [PayloadAttributes].
    PayloadAttributes(&'a PayloadAttributes),
}

impl<'a> PayloadOrAttributes<'a> {
    /// Construct a [PayloadOrAttributes] from an [ExecutionPayload] and optional parent beacon
    /// block root.
    pub(crate) fn from_execution_payload(
        payload: &'a ExecutionPayload,
        parent_beacon_block_root: Option<H256>,
    ) -> Self {
        Self::ExecutionPayload { payload, parent_beacon_block_root }
    }

    /// Return the withdrawals for the payload or attributes.
    pub(crate) fn withdrawals(&self) -> &Option<Vec<Withdrawal>> {
        match self {
            Self::ExecutionPayload { payload, .. } => &payload.withdrawals,
            Self::PayloadAttributes(attributes) => &attributes.withdrawals,
        }
    }

    /// Return the timestamp for the payload or attributes.
    pub(crate) fn timestamp(&self) -> u64 {
        match self {
            Self::ExecutionPayload { payload, .. } => payload.timestamp.as_u64(),
            Self::PayloadAttributes(attributes) => attributes.timestamp.as_u64(),
        }
    }

    /// Return the parent beacon block root for the payload or attributes.
    pub(crate) fn parent_beacon_block_root(&self) -> Option<H256> {
        match self {
            Self::ExecutionPayload { parent_beacon_block_root, .. } => *parent_beacon_block_root,
            Self::PayloadAttributes(attributes) => attributes.parent_beacon_block_root,
        }
    }
}

impl<'a> From<&'a PayloadAttributes> for PayloadOrAttributes<'a> {
    fn from(attributes: &'a PayloadAttributes) -> Self {
        Self::PayloadAttributes(attributes)
    }
}
