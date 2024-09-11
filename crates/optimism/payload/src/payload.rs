//! Payload related types

//! Optimism builder support

use alloy_primitives::{Address, B256, U256};
use alloy_rlp::Encodable;
use reth_chain_state::ExecutedBlock;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_evm_optimism::revm_spec_by_timestamp_after_bedrock;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes};
use reth_primitives::{
    revm_primitives::{BlobExcessGasAndPrice, BlockEnv, CfgEnv, CfgEnvWithHandlerCfg, SpecId},
    transaction::WithEncoded,
    BlobTransactionSidecar, Header, SealedBlock, TransactionSigned, Withdrawals,
};
/// Re-export for use in downstream arguments.
pub use reth_rpc_types::optimism::OptimismPayloadAttributes;
use reth_rpc_types::{
    engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1, PayloadId},
    optimism::{OptimismExecutionPayloadEnvelopeV3, OptimismExecutionPayloadEnvelopeV4},
};
use reth_rpc_types_compat::engine::payload::{
    block_to_payload_v1, block_to_payload_v3, block_to_payload_v4,
    convert_block_to_payload_field_v2,
};
use revm::primitives::HandlerCfg;
use std::sync::Arc;

/// Optimism Payload Builder Attributes
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimismPayloadBuilderAttributes {
    /// Inner ethereum payload builder attributes
    pub payload_attributes: EthPayloadBuilderAttributes,
    /// `NoTxPool` option for the generated payload
    pub no_tx_pool: bool,
    /// Decoded transactions and the original EIP-2718 encoded bytes as received in the payload
    /// attributes.
    pub transactions: Vec<WithEncoded<TransactionSigned>>,
    /// The gas limit for the generated payload
    pub gas_limit: Option<u64>,
}

impl PayloadBuilderAttributes for OptimismPayloadBuilderAttributes {
    type RpcPayloadAttributes = OptimismPayloadAttributes;
    type Error = alloy_rlp::Error;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [`PayloadId`] for the given parent and attributes
    fn try_new(parent: B256, attributes: OptimismPayloadAttributes) -> Result<Self, Self::Error> {
        let id = payload_id_optimism(&parent, &attributes);

        let transactions = attributes
            .transactions
            .unwrap_or_default()
            .into_iter()
            .map(|data| {
                TransactionSigned::decode_enveloped(&mut data.as_ref())
                    .map(|tx| WithEncoded::new(data, tx))
            })
            .collect::<Result<_, _>>()?;

        let payload_attributes = EthPayloadBuilderAttributes {
            id,
            parent,
            timestamp: attributes.payload_attributes.timestamp,
            suggested_fee_recipient: attributes.payload_attributes.suggested_fee_recipient,
            prev_randao: attributes.payload_attributes.prev_randao,
            withdrawals: attributes.payload_attributes.withdrawals.unwrap_or_default().into(),
            parent_beacon_block_root: attributes.payload_attributes.parent_beacon_block_root,
        };

        Ok(Self {
            payload_attributes,
            no_tx_pool: attributes.no_tx_pool.unwrap_or_default(),
            transactions,
            gas_limit: attributes.gas_limit,
        })
    }

    fn payload_id(&self) -> PayloadId {
        self.payload_attributes.id
    }

    fn parent(&self) -> B256 {
        self.payload_attributes.parent
    }

    fn timestamp(&self) -> u64 {
        self.payload_attributes.timestamp
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.payload_attributes.parent_beacon_block_root
    }

    fn suggested_fee_recipient(&self) -> Address {
        self.payload_attributes.suggested_fee_recipient
    }

    fn prev_randao(&self) -> B256 {
        self.payload_attributes.prev_randao
    }

    fn withdrawals(&self) -> &Withdrawals {
        &self.payload_attributes.withdrawals
    }

    fn cfg_and_block_env(
        &self,
        chain_spec: &ChainSpec,
        parent: &Header,
    ) -> (CfgEnvWithHandlerCfg, BlockEnv) {
        // configure evm env based on parent block
        let cfg = CfgEnv::default().with_chain_id(chain_spec.chain().id());

        // ensure we're not missing any timestamp based hardforks
        let spec_id = revm_spec_by_timestamp_after_bedrock(chain_spec, self.timestamp());

        // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
        // cancun now, we need to set the excess blob gas to the default value
        let blob_excess_gas_and_price = parent
            .next_block_excess_blob_gas()
            .or_else(|| {
                if spec_id.is_enabled_in(SpecId::CANCUN) {
                    // default excess blob gas is zero
                    Some(0)
                } else {
                    None
                }
            })
            .map(BlobExcessGasAndPrice::new);

        let block_env = BlockEnv {
            number: U256::from(parent.number + 1),
            coinbase: self.suggested_fee_recipient(),
            timestamp: U256::from(self.timestamp()),
            difficulty: U256::ZERO,
            prevrandao: Some(self.prev_randao()),
            gas_limit: U256::from(parent.gas_limit),
            // calculate basefee based on parent block's gas usage
            basefee: U256::from(
                parent
                    .next_block_base_fee(chain_spec.base_fee_params_at_timestamp(self.timestamp()))
                    .unwrap_or_default(),
            ),
            // calculate excess gas based on parent block's blob gas usage
            blob_excess_gas_and_price,
        };

        let cfg_with_handler_cfg;
        {
            cfg_with_handler_cfg = CfgEnvWithHandlerCfg {
                cfg_env: cfg,
                handler_cfg: HandlerCfg { spec_id, is_optimism: true },
            };
        }

        (cfg_with_handler_cfg, block_env)
    }
}

/// Contains the built payload.
#[derive(Debug, Clone)]
pub struct OptimismBuiltPayload {
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
    /// The rollup's chainspec.
    pub(crate) chain_spec: Arc<ChainSpec>,
    /// The payload attributes.
    pub(crate) attributes: OptimismPayloadBuilderAttributes,
}

// === impl BuiltPayload ===

impl OptimismBuiltPayload {
    /// Initializes the payload with the given initial block.
    pub const fn new(
        id: PayloadId,
        block: SealedBlock,
        fees: U256,
        chain_spec: Arc<ChainSpec>,
        attributes: OptimismPayloadBuilderAttributes,
        executed_block: Option<ExecutedBlock>,
    ) -> Self {
        Self { id, block, executed_block, fees, sidecars: Vec::new(), chain_spec, attributes }
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

    /// Adds sidecars to the payload.
    pub fn extend_sidecars(&mut self, sidecars: Vec<BlobTransactionSidecar>) {
        self.sidecars.extend(sidecars)
    }
}

impl BuiltPayload for OptimismBuiltPayload {
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

impl<'a> BuiltPayload for &'a OptimismBuiltPayload {
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
impl From<OptimismBuiltPayload> for ExecutionPayloadV1 {
    fn from(value: OptimismBuiltPayload) -> Self {
        block_to_payload_v1(value.block)
    }
}

// V2 engine_getPayloadV2 response
impl From<OptimismBuiltPayload> for ExecutionPayloadEnvelopeV2 {
    fn from(value: OptimismBuiltPayload) -> Self {
        let OptimismBuiltPayload { block, fees, .. } = value;

        Self { block_value: fees, execution_payload: convert_block_to_payload_field_v2(block) }
    }
}

impl From<OptimismBuiltPayload> for OptimismExecutionPayloadEnvelopeV3 {
    fn from(value: OptimismBuiltPayload) -> Self {
        let OptimismBuiltPayload { block, fees, sidecars, chain_spec, attributes, .. } = value;

        let parent_beacon_block_root =
            if chain_spec.is_cancun_active_at_timestamp(attributes.timestamp()) {
                attributes.parent_beacon_block_root().unwrap_or(B256::ZERO)
            } else {
                B256::ZERO
            };
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
            parent_beacon_block_root,
        }
    }
}
impl From<OptimismBuiltPayload> for OptimismExecutionPayloadEnvelopeV4 {
    fn from(value: OptimismBuiltPayload) -> Self {
        let OptimismBuiltPayload { block, fees, sidecars, chain_spec, attributes, .. } = value;

        let parent_beacon_block_root =
            if chain_spec.is_cancun_active_at_timestamp(attributes.timestamp()) {
                attributes.parent_beacon_block_root().unwrap_or(B256::ZERO)
            } else {
                B256::ZERO
            };
        Self {
            execution_payload: block_to_payload_v4(block),
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
            parent_beacon_block_root,
        }
    }
}

/// Generates the payload id for the configured payload from the [`OptimismPayloadAttributes`].
///
/// Returns an 8-byte identifier by hashing the payload components with sha256 hash.
pub(crate) fn payload_id_optimism(
    parent: &B256,
    attributes: &OptimismPayloadAttributes,
) -> PayloadId {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(parent.as_slice());
    hasher.update(&attributes.payload_attributes.timestamp.to_be_bytes()[..]);
    hasher.update(attributes.payload_attributes.prev_randao.as_slice());
    hasher.update(attributes.payload_attributes.suggested_fee_recipient.as_slice());
    if let Some(withdrawals) = &attributes.payload_attributes.withdrawals {
        let mut buf = Vec::new();
        withdrawals.encode(&mut buf);
        hasher.update(buf);
    }

    if let Some(parent_beacon_block) = attributes.payload_attributes.parent_beacon_block_root {
        hasher.update(parent_beacon_block);
    }

    let no_tx_pool = attributes.no_tx_pool.unwrap_or_default();
    hasher.update([no_tx_pool as u8]);
    if let Some(txs) = &attributes.transactions {
        txs.iter().for_each(|tx| hasher.update(tx));
    }

    if let Some(gas_limit) = attributes.gas_limit {
        hasher.update(gas_limit.to_be_bytes());
    }

    let out = hasher.finalize();
    PayloadId::new(out.as_slice()[..8].try_into().expect("sufficient length"))
}
