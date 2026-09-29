//! Types and traits for execution payload data structures.

use crate::{MessageValidationKind, PayloadAttributes};
use alloc::vec::Vec;
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal, eip7685::Requests, BlockNumHash};
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::ExecutionData;
use core::fmt::Debug;
use serde::{de::DeserializeOwned, Serialize};
use tracing::Span;

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
    fn withdrawals(&self) -> Option<&Vec<Withdrawal>>;

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

    /// Returns the total gas limit for this block.
    fn gas_limit(&self) -> u64;

    /// Returns the number of transactions in the payload.
    fn transaction_count(&self) -> usize;
    /// Returns the slot number included in this payload.
    ///
    /// Returns `None` for pre-Amsterdam blocks.
    fn slot_number(&self) -> Option<u64>;

    /// Returns the caller's span to use as the parent of engine payload processing.
    ///
    /// Implementations may capture a span before sending the payload to the engine to preserve
    /// tracing context across the channel. This is local metadata and should not be serialized.
    /// The default returns `None`, making the engine's `on_new_payload` span a root span.
    fn cause(&self) -> Option<&Span> {
        None
    }
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

    fn block_access_list(&self) -> Option<&Bytes> {
        self.payload.block_access_list()
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

    fn gas_limit(&self) -> u64 {
        self.payload.as_v1().gas_limit
    }

    fn transaction_count(&self) -> usize {
        self.payload.as_v1().transactions.len()
    }

    fn slot_number(&self) -> Option<u64> {
        self.payload.slot_number()
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
    pub fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
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

    /// Returns `block_access_list` from  payload.
    pub fn block_access_list(&self) -> Option<&Bytes> {
        match self {
            Self::ExecutionPayload(payload) => payload.block_access_list(),
            Self::PayloadAttributes(_attributes) => None,
        }
    }

    /// Returns `slot_number` from  payload or attributes.
    pub fn slot_number(&self) -> Option<u64> {
        match self {
            Self::ExecutionPayload(payload) => payload.slot_number(),
            Self::PayloadAttributes(attributes) => attributes.slot_number(),
        }
    }

    /// Returns `target_gas_limit` from payload attributes.
    pub fn target_gas_limit(&self) -> Option<u64> {
        match self {
            Self::ExecutionPayload(_) => None,
            Self::PayloadAttributes(attributes) => attributes.target_gas_limit(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Block, TxEnvelope};
    use alloy_rpc_types_engine::{ExecutionPayloadSidecar, ExecutionPayloadV1};
    use tracing::{span::Id, Dispatch};
    use tracing_subscriber::{registry::LookupSpan, Registry};

    #[derive(Debug, Clone, Serialize, serde::Deserialize)]
    struct TracedPayload {
        inner: ExecutionData,
        #[serde(skip)]
        cause: Option<Span>,
    }

    // Forward the ordinary payload fields so the test only customizes tracing context.
    macro_rules! delegate {
        ($($name:ident -> $ret:ty),* $(,)?) => {
            $(fn $name(&self) -> $ret { self.inner.$name() })*
        };
    }

    impl ExecutionPayload for TracedPayload {
        delegate! {
            parent_hash -> B256,
            block_hash -> B256,
            block_number -> u64,
            withdrawals -> Option<&Vec<Withdrawal>>,
            block_access_list -> Option<&Bytes>,
            parent_beacon_block_root -> Option<B256>,
            timestamp -> u64,
            gas_used -> u64,
            gas_limit -> u64,
            transaction_count -> usize,
            slot_number -> Option<u64>,
        }

        fn cause(&self) -> Option<&Span> {
            self.cause.as_ref()
        }
    }

    #[tracing::instrument(
        parent = payload.cause().and_then(Span::id),
        target = "engine::tree",
        skip_all,
    )]
    fn process_payload(payload: impl ExecutionPayload) -> Option<Id> {
        Span::current()
            .with_subscriber(|(id, dispatch)| {
                dispatch
                    .downcast_ref::<Registry>()
                    .unwrap()
                    .span(id)
                    .unwrap()
                    .parent()
                    .map(|parent| parent.id())
            })
            .unwrap()
    }

    fn execution_data() -> ExecutionData {
        ExecutionData {
            payload: ExecutionPayloadV1::from_block_unchecked(
                B256::ZERO,
                &Block::<TxEnvelope>::default(),
            )
            .into(),
            sidecar: ExecutionPayloadSidecar::default(),
        }
    }

    #[test]
    fn payload_cause_survives_channel_handoff() {
        let dispatch = Dispatch::new(Registry::default());
        let (tx, rx) = std::sync::mpsc::channel();
        let cause = tracing::dispatcher::with_default(&dispatch, || {
            let cause = tracing::info_span!("caller");
            tx.send(TracedPayload { inner: execution_data(), cause: Some(cause.clone()) }).unwrap();
            cause
        });

        let parent = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let _unrelated = tracing::info_span!("engine").entered();
                process_payload(rx.recv().unwrap())
            })
        })
        .join()
        .unwrap();
        assert_eq!(parent, cause.id());
        assert!(parent.is_some());
    }

    #[test]
    fn default_and_deserialized_payloads_have_no_parent() {
        tracing::subscriber::with_default(Registry::default(), || {
            let cause = tracing::info_span!("caller");
            let payload = TracedPayload { inner: execution_data(), cause: Some(cause) };
            let encoded = serde_json::to_value(payload).unwrap();
            assert!(encoded.get("cause").is_none());
            let decoded = serde_json::from_value::<TracedPayload>(encoded).unwrap();

            let _unrelated = tracing::info_span!("engine").entered();
            assert_eq!(process_payload(execution_data()), None);
            assert_eq!(process_payload(decoded), None);
        });
    }
}
