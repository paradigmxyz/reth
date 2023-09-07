//! Contains types required for building a payload.

use reth_primitives::{
    Address, BlobTransactionSidecar, ChainSpec, Header, SealedBlock, Withdrawal, H256, U256,
};
use reth_revm_primitives::config::revm_spec_by_timestamp_after_merge;
use reth_rlp::Encodable;
use reth_rpc_types::engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadV1, PayloadAttributes,
    PayloadId,
};
use revm_primitives::{BlockEnv, CfgEnv};

/// Contains the built payload.
///
/// According to the [engine API specification](https://github.com/ethereum/execution-apis/blob/main/src/engine/README.md) the execution layer should build the initial version of the payload with an empty transaction set and then keep update it in order to maximize the revenue.
/// Therefore, the empty-block here is always available and full-block will be set/updated
/// afterwards.
#[derive(Debug, Clone)]
pub struct BuiltPayload {
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

impl BuiltPayload {
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

// V1 engine_getPayloadV1 response
impl From<BuiltPayload> for ExecutionPayloadV1 {
    fn from(value: BuiltPayload) -> Self {
        value.block.into()
    }
}

// V2 engine_getPayloadV2 response
impl From<BuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: BuiltPayload) -> Self {
        let BuiltPayload { block, fees, .. } = value;

        ExecutionPayloadEnvelopeV2 { block_value: fees, execution_payload: block.into() }
    }
}

impl From<BuiltPayload> for ExecutionPayloadEnvelopeV3 {
    fn from(value: BuiltPayload) -> Self {
        let BuiltPayload { block, fees, sidecars, .. } = value;

        ExecutionPayloadEnvelopeV3 {
            execution_payload: block.into(),
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
            blobs_bundle: sidecars.into(),
        }
    }
}

/// Container type for all components required to build a payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadBuilderAttributes {
    /// Id of the payload
    pub id: PayloadId,
    /// Parent block to build the payload on top
    pub parent: H256,
    /// Timestamp for the generated payload
    pub timestamp: u64,
    /// Address of the recipient for collecting transaction fee
    pub suggested_fee_recipient: Address,
    /// Randomness value for the generated payload
    pub prev_randao: H256,
    /// Withdrawals for the generated payload
    pub withdrawals: Vec<Withdrawal>,
    /// Root of the parent beacon block
    pub parent_beacon_block_root: Option<H256>,
}

// === impl PayloadBuilderAttributes ===

impl PayloadBuilderAttributes {
    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    pub fn new(parent: H256, attributes: PayloadAttributes) -> Self {
        let id = payload_id(&parent, &attributes);
        Self {
            id,
            parent,
            timestamp: attributes.timestamp.as_u64(),
            suggested_fee_recipient: attributes.suggested_fee_recipient,
            prev_randao: attributes.prev_randao,
            withdrawals: attributes.withdrawals.unwrap_or_default(),
            parent_beacon_block_root: attributes.parent_beacon_block_root,
        }
    }

    /// Returns the configured [CfgEnv] and [BlockEnv] for the targeted payload (that has the
    /// `parent` as its parent).
    ///
    /// The `chain_spec` is used to determine the correct chain id and hardfork for the payload
    /// based on its timestamp.
    ///
    /// Block related settings are derived from the `parent` block and the configured attributes.
    ///
    /// NOTE: This is only intended for beacon consensus (after merge).
    pub fn cfg_and_block_env(&self, chain_spec: &ChainSpec, parent: &Header) -> (CfgEnv, BlockEnv) {
        // configure evm env based on parent block
        let cfg = CfgEnv {
            chain_id: U256::from(chain_spec.chain().id()),
            // ensure we're not missing any timestamp based hardforks
            spec_id: revm_spec_by_timestamp_after_merge(chain_spec, self.timestamp),
            ..Default::default()
        };

        let block_env = BlockEnv {
            number: U256::from(parent.number + 1),
            coinbase: self.suggested_fee_recipient,
            timestamp: U256::from(self.timestamp),
            difficulty: U256::ZERO,
            prevrandao: Some(self.prev_randao),
            gas_limit: U256::from(parent.gas_limit),
            // calculate basefee based on parent block's gas usage
            basefee: U256::from(
                parent.next_block_base_fee(chain_spec.base_fee_params).unwrap_or_default(),
            ),
        };

        (cfg, block_env)
    }

    /// Returns the identifier of the payload.
    pub fn payload_id(&self) -> PayloadId {
        self.id
    }
}

/// Generates the payload id for the configured payload
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id(parent: &H256, attributes: &PayloadAttributes) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(&attributes.timestamp.as_u64().to_be_bytes()[..]);
    hasher.update(attributes.prev_randao.as_bytes());
    hasher.update(attributes.suggested_fee_recipient.as_bytes());
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
