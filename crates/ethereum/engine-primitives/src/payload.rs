//! Contains types required for building a payload.

use alloy_eips::{eip4844::BlobTransactionSidecar, eip7685::Requests};
use alloy_primitives::{Address, B256, U256};
use alloy_rlp::Encodable;
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadV1, PayloadAttributes, PayloadId,
};
use reth_chain_state::ExecutedBlock;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes};
use reth_primitives::{SealedBlock, Withdrawals};
use reth_rpc_types_compat::engine::payload::{
    block_to_payload_v1, block_to_payload_v3, convert_block_to_payload_field_v2,
};
use std::convert::Infallible;

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
/// Therefore, the empty-block here is always available and full-block will be set/updated
/// afterward.
#[derive(Debug, Clone)]
pub struct EthBuiltPayload {
    /// Identifier of the payload
    pub(crate) id: PayloadId,
    /// The built block
    pub(crate) block: SealedBlock,
    /// Block execution data for the payload, if any.
    pub(crate) executed_block: Option<ExecutedBlock>,
    /// The fees of the block
    pub(crate) fees: U256,
    /// The blobs, proofs, and commitments in the block. If the block is pre-cancun, this will be
    /// empty.
    pub(crate) sidecars: Vec<BlobTransactionSidecar>,
}

// === impl BuiltPayload ===

impl EthBuiltPayload {
    /// Initializes the payload with the given initial block.
    pub const fn new(
        id: PayloadId,
        block: SealedBlock,
        fees: U256,
        executed_block: Option<ExecutedBlock>,
    ) -> Self {
        Self { id, block, executed_block, fees, sidecars: Vec::new() }
    }

    /// Returns the identifier of the payload.
    pub const fn id(&self) -> PayloadId {
        self.id
    }

    /// Returns the built block(sealed)
    pub const fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Fees of the block
    pub const fn fees(&self) -> U256 {
        self.fees
    }

    /// Returns the blob sidecars.
    pub fn sidecars(&self) -> &[BlobTransactionSidecar] {
        &self.sidecars
    }

    /// Adds sidecars to the payload.
    pub fn extend_sidecars(&mut self, sidecars: Vec<BlobTransactionSidecar>) {
        self.sidecars.extend(sidecars)
    }
}

impl BuiltPayload for EthBuiltPayload {
    fn block(&self) -> &SealedBlock {
        &self.block
    }

    fn fees(&self) -> U256 {
        self.fees
    }

    fn executed_block(&self) -> Option<ExecutedBlock> {
        self.executed_block.clone()
    }
}

impl BuiltPayload for &EthBuiltPayload {
    fn block(&self) -> &SealedBlock {
        (**self).block()
    }

    fn fees(&self) -> U256 {
        (**self).fees()
    }

    fn executed_block(&self) -> Option<ExecutedBlock> {
        self.executed_block.clone()
    }
}

// V1 engine_getPayloadV1 response
impl From<EthBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: EthBuiltPayload) -> Self {
        block_to_payload_v1(value.block)
    }
}

// V2 engine_getPayloadV2 response
impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, .. } = value;

        Self { block_value: fees, execution_payload: convert_block_to_payload_field_v2(block) }
    }
}

impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, sidecars, .. } = value;

        Self {
            execution_payload: block_to_payload_v3(block),
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
            blobs_bundle: sidecars.into_iter().map(Into::into).collect::<Vec<_>>().into(),
        }
    }
}

impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV4 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, sidecars, executed_block, .. } = value;

        // if we have an executed block, we pop off the first set of requests from the execution
        // outcome. the assumption here is that there will always only be one block in the execution
        // outcome.
        let execution_requests = executed_block
            .and_then(|block| block.execution_outcome().requests.first().cloned())
            .map(Requests::take)
            .unwrap_or_default();
        Self {
            execution_payload: block_to_payload_v3(block),
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
            blobs_bundle: sidecars.into_iter().map(Into::into).collect::<Vec<_>>().into(),
            execution_requests,
        }
    }
}

/// Container type for all components required to build a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EthPayloadBuilderAttributes {
    /// Id of the payload
    pub id: PayloadId,
    /// Parent block to build the payload on top
    pub parent: B256,
    /// Unix timestamp for the generated payload
    ///
    /// Number of seconds since the Unix epoch.
    pub timestamp: u64,
    /// Address of the recipient for collecting transaction fee
    pub suggested_fee_recipient: Address,
    /// Randomness value for the generated payload
    pub prev_randao: B256,
    /// Withdrawals for the generated payload
    pub withdrawals: Withdrawals,
    /// Root of the parent beacon block
    pub parent_beacon_block_root: Option<B256>,
}

// === impl EthPayloadBuilderAttributes ===

impl EthPayloadBuilderAttributes {
    /// Returns the identifier of the payload.
    pub const fn payload_id(&self) -> PayloadId {
        self.id
    }

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    pub fn new(parent: B256, attributes: PayloadAttributes) -> Self {
        let id = payload_id(&parent, &attributes);

        Self {
            id,
            parent,
            timestamp: attributes.timestamp,
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: attributes.withdrawals.unwrap_or_default().into(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
        }
    }
}

impl PayloadBuilderAttributes for EthPayloadBuilderAttributes {
    type RpcPayloadAttributes = PayloadAttributes;
    type Error = Infallible;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    fn try_new(parent: B256, attributes: PayloadAttributes) -> Result<Self, Infallible> {
        Ok(Self::new(parent, attributes))
    }

    fn payload_id(&self) -> PayloadId {
        self.id
    }

    fn parent(&self) -> B256 {
        self.parent
    }

    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.prev_randao
    }

    fn withdrawals(&self) -> &Withdrawals {
        &self.withdrawals
    }
}

/// Generates the payload id for the configured payload from the [`PayloadAttributes`].
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id(parent: &B256, attributes: &PayloadAttributes) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_slice());
    hasher.update(&attributes.timestamp.to_be_bytes()[..]);
    hasher.update(attributes.prev_randao.as_slice());
    hasher.update(attributes.suggested_fee_recipient.as_slice());
    if let Some(withdrawals) = &attributes.withdrawals {
        let mut buf = Vec::new();
        withdrawals.encode(&mut buf);
        hasher.update(buf);
    }

    if let Some(parent_beacon_block) = attributes.parent_beacon_block_root {
        hasher.update(parent_beacon_block);
    }

    let out = hasher.finalize();
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_serde() {
        let attributes = r#"{"timestamp":"0x1235","prevRandao":"0xf343b00e02dc34ec0124241f74f32191be28fb370bb48060f5fa4df99bda774c","suggestedFeeRecipient":"0x0000000000000000000000000000000000000000","withdrawals":null,"parentBeaconBlockRoot":null}"#;
        let _attributes: PayloadAttributes = serde_json::from_str(attributes).unwrap();
    }
}
