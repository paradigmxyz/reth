//! Contains types required for building a payload.

use alloy_rlp::Encodable;
use reth_node_api::{BuiltPayload, PayloadBuilderAttributes};
use reth_primitives::{Address, BlobTransactionSidecar, SealedBlock, Withdrawal, B256, U256};
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadV1, PayloadAttributes,
    PayloadId,
};
use reth_rpc_types_compat::engine::payload::{
    block_to_payload_v3, convert_block_to_payload_field_v2,
    convert_standalone_withdraw_to_withdrawal, try_block_to_payload_v1,
};
use std::convert::Infallible;

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
/// Therefore, the empty-block here is always available and full-block will be set/updated
/// afterwards.
#[derive(Debug, Clone)]
pub struct EthBuiltPayload {
    /// Identifier of the payload
    pub(crate) id: PayloadId,
    /// The built block
    pub(crate) block: SealedBlock,
    /// The fees of the block
    pub(crate) fees: U256,
    /// The blobs, proofs, and commitments in the block. If the block is pre-cancun, this will be
    /// empty.
    pub(crate) sidecars: Vec<BlobTransactionSidecar>,
}

// === impl BuiltPayload ===

impl EthBuiltPayload {
    /// Initializes the payload with the given initial block.
    pub fn new(id: PayloadId, block: SealedBlock, fees: U256) -> Self {
        Self { id, block, fees, sidecars: Vec::new() }
    }

    /// Returns the identifier of the payload.
    pub fn id(&self) -> PayloadId {
        self.id
    }

    /// Returns the built block(sealed)
    pub fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Fees of the block
    pub fn fees(&self) -> U256 {
        self.fees
    }

    /// Adds sidecars to the payload.
    pub fn extend_sidecars(&mut self, sidecars: Vec<BlobTransactionSidecar>) {
        self.sidecars.extend(sidecars)
    }

    /// Converts the type into the response expected by `engine_getPayloadV1`
    pub fn into_v1_payload(self) -> ExecutionPayloadV1 {
        self.into()
    }

    /// Converts the type into the response expected by `engine_getPayloadV2`
    pub fn into_v2_payload(self) -> ExecutionPayloadEnvelopeV2 {
        self.into()
    }

    /// Converts the type into the response expected by `engine_getPayloadV2`
    pub fn into_v3_payload(self) -> ExecutionPayloadEnvelopeV3 {
        self.into()
    }
}

impl BuiltPayload for EthBuiltPayload {
    fn block(&self) -> &SealedBlock {
        &self.block
    }

    fn fees(&self) -> U256 {
        self.fees
    }

    fn into_v1_payload(self) -> ExecutionPayloadV1 {
        self.into()
    }

    fn into_v2_payload(self) -> ExecutionPayloadEnvelopeV2 {
        self.into()
    }

    fn into_v3_payload(self) -> ExecutionPayloadEnvelopeV3 {
        self.into()
    }
}

// V1 engine_getPayloadV1 response
impl From<EthBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: EthBuiltPayload) -> Self {
        try_block_to_payload_v1(value.block)
    }
}

// V2 engine_getPayloadV2 response
impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, .. } = value;

        ExecutionPayloadEnvelopeV2 {
            block_value: fees,
            execution_payload: convert_block_to_payload_field_v2(block),
        }
    }
}

impl From<EthBuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: EthBuiltPayload) -> Self {
        let EthBuiltPayload { block, fees, sidecars, .. } = value;

        ExecutionPayloadEnvelopeV3 {
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
    pub withdrawals: Vec<Withdrawal>,
    /// Root of the parent beacon block
    pub parent_beacon_block_root: Option<B256>,
}

// === impl EthPayloadBuilderAttributes ===

impl EthPayloadBuilderAttributes {
    /// Returns the identifier of the payload.
    pub fn payload_id(&self) -> PayloadId {
        self.id
    }

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    pub fn new(parent: B256, attributes: PayloadAttributes) -> Self {
        let id = payload_id(&parent, &attributes);

        let withdraw = attributes.withdrawals.map(
            |withdrawals: Vec<reth_rpc_types::withdrawal::Withdrawal>| {
                withdrawals
                    .into_iter()
                    .map(convert_standalone_withdraw_to_withdrawal) // Removed the parentheses here
                    .collect::<Vec<_>>()
            },
        );

        Self {
            id,
            parent,
            timestamp: attributes.timestamp,
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: withdraw.unwrap_or_default(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
        }
    }
}

impl PayloadBuilderAttributes for EthPayloadBuilderAttributes {
    type RpcPayloadAttributes = PayloadAttributes;
    type Error = Infallible;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    fn try_new(parent: B256, attributes: PayloadAttributes) -> Result<Self, Infallible> {
        Ok(Self::new(parent, attributes))
    }

    fn parent(&self) -> B256 {
        self.parent
    }

    fn payload_id(&self) -> PayloadId {
        self.id
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

    fn withdrawals(&self) -> &Vec<Withdrawal> {
        &self.withdrawals
    }
}

/// Generates the payload id for the configured payload from the [PayloadAttributes].
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
