//! Outcome of a Scroll block building task with payload attributes provided via the Engine API.

use alloc::{iter, sync::Arc};

use alloy_eips::eip7685::Requests;
use alloy_primitives::U256;
use alloy_rpc_types_engine::{
    BlobsBundleV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadV1, PayloadId,
};
use reth_chain_state::ExecutedBlock;
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::SealedBlock;
use reth_rpc_types_compat::engine::{
    block_to_payload_v1,
    payload::{block_to_payload_v3, convert_block_to_payload_field_v2},
};
use reth_scroll_primitives::{ScrollBlock, ScrollPrimitives};

/// Contains the built payload.
#[derive(Debug, Clone, Default)]
pub struct ScrollBuiltPayload {
    /// Identifier of the payload
    pub(crate) id: PayloadId,
    /// The built block
    pub(crate) block: Arc<SealedBlock<ScrollBlock>>,
    /// Block execution data for the payload, if any.
    pub(crate) executed_block: Option<ExecutedBlock<ScrollPrimitives>>,
    /// The fees of the block
    pub(crate) fees: U256,
}

impl ScrollBuiltPayload {
    /// Initializes the payload with the given initial block.
    pub const fn new(
        id: PayloadId,
        block: Arc<reth_primitives::SealedBlock<ScrollBlock>>,
        fees: U256,
        executed_block: Option<ExecutedBlock<ScrollPrimitives>>,
    ) -> Self {
        Self { id, block, executed_block, fees }
    }

    /// Returns the identifier of the payload.
    pub const fn id(&self) -> PayloadId {
        self.id
    }

    /// Returns the built block(sealed)
    #[allow(clippy::missing_const_for_fn)]
    pub fn block(&self) -> &reth_primitives::SealedBlock<ScrollBlock> {
        &self.block
    }

    /// Fees of the block
    pub const fn fees(&self) -> U256 {
        self.fees
    }
}

impl BuiltPayload for ScrollBuiltPayload {
    type Primitives = ScrollPrimitives;

    fn block(&self) -> &SealedBlock<ScrollBlock> {
        &self.block
    }

    fn fees(&self) -> U256 {
        self.fees
    }

    fn executed_block(&self) -> Option<ExecutedBlock<ScrollPrimitives>> {
        self.executed_block.clone()
    }

    fn requests(&self) -> Option<Requests> {
        None
    }
}

impl BuiltPayload for &ScrollBuiltPayload {
    type Primitives = ScrollPrimitives;

    fn block(&self) -> &SealedBlock<ScrollBlock> {
        (**self).block()
    }

    fn fees(&self) -> U256 {
        (**self).fees()
    }

    fn executed_block(&self) -> Option<ExecutedBlock<ScrollPrimitives>> {
        self.executed_block.clone()
    }

    fn requests(&self) -> Option<Requests> {
        None
    }
}

// V1 engine_getPayloadV1 response
impl From<ScrollBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: ScrollBuiltPayload) -> Self {
        block_to_payload_v1(Arc::unwrap_or_clone(value.block))
    }
}

// V2 engine_getPayloadV2 response
impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: ScrollBuiltPayload) -> Self {
        let ScrollBuiltPayload { block, fees, .. } = value;

        Self {
            block_value: fees,
            execution_payload: convert_block_to_payload_field_v2(Arc::unwrap_or_clone(block)),
        }
    }
}

impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: ScrollBuiltPayload) -> Self {
        let ScrollBuiltPayload { block, fees, .. } = value;

        Self {
            execution_payload: block_to_payload_v3(Arc::unwrap_or_clone(block)),
            block_value: fees,
            // From the engine API spec:
            //
            // > Client software **MAY** use any heuristics to decide whether to set
            // `shouldOverrideBuilder` flag or not. If client software does not implement any
            // heuristic this flag **SHOULD** be set to `false`.
            //
            // Spec:
            // <https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md#specification-2>
            should_override_builder: false,
            blobs_bundle: BlobsBundleV1::new(iter::empty()),
        }
    }
}
impl From<ScrollBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    fn from(value: ScrollBuiltPayload) -> Self {
        Self { envelope_inner: value.into(), execution_requests: Default::default() }
    }
}
