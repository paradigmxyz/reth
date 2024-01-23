use crate::{validate_version_specific_fields, AttributesValidationError, EngineApiMessageVersion};
use reth_primitives::{
    revm::config::revm_spec_by_timestamp_after_merge,
    revm_primitives::{BlobExcessGasAndPrice, BlockEnv, CfgEnv, SpecId},
    Address, ChainSpec, Header, SealedBlock, B256, U256,
};
use reth_rpc_types::{
    engine::{
        ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, OptimismPayloadAttributes,
        PayloadAttributes as EthPayloadAttributes, PayloadId,
    },
    withdrawal::Withdrawal,
    ExecutionPayloadV1,
};

/// Represents a built payload type that contains a built [SealedBlock] and can be converted into
/// engine API execution payloads.
pub trait BuiltPayload: Send + Sync + std::fmt::Debug {
    /// Returns the built block (sealed)
    fn block(&self) -> &SealedBlock;

    /// Returns the fees collected for the built block
    fn fees(&self) -> U256;

    /// Converts the type into the response expected by `engine_getPayloadV1`
    fn into_v1_payload(self) -> ExecutionPayloadV1;

    /// Converts the type into the response expected by `engine_getPayloadV2`
    fn into_v2_payload(self) -> ExecutionPayloadEnvelopeV2;

    /// Converts the type into the response expected by `engine_getPayloadV2`
    fn into_v3_payload(self) -> ExecutionPayloadEnvelopeV3;
}

/// This can be implemented by types that describe a currently running payload job.
///
/// This is used as a conversion type, transforming a payload attributes type that the engine API
/// receives, into a type that the payload builder can use.
pub trait PayloadBuilderAttributes: Send + Sync + std::fmt::Debug {
    /// The payload attributes that can be used to construct this type. Used as the argument in
    /// [PayloadBuilderAttributes::try_new].
    type RpcPayloadAttributes;
    /// The error type used in [PayloadBuilderAttributes::try_new].
    type Error: std::error::Error;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    fn try_new(
        parent: B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;

    /// Returns the [PayloadId] for the running payload job.
    fn payload_id(&self) -> PayloadId;

    /// Returns the parent block hash for the running payload job.
    fn parent(&self) -> B256;

    /// Returns the timestmap for the running payload job.
    fn timestamp(&self) -> u64;

    /// Returns the parent beacon block root for the running payload job, if it exists.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Returns the suggested fee recipient for the running payload job.
    fn suggested_fee_recipient(&self) -> Address;

    /// Returns the prevrandao field for the running payload job.
    fn prev_randao(&self) -> B256;

    /// Returns the withdrawals for the running payload job.
    fn withdrawals(&self) -> &Vec<reth_primitives::Withdrawal>;

    /// Returns the configured [CfgEnv] and [BlockEnv] for the targeted payload (that has the
    /// `parent` as its parent).
    ///
    /// The `chain_spec` is used to determine the correct chain id and hardfork for the payload
    /// based on its timestamp.
    ///
    /// Block related settings are derived from the `parent` block and the configured attributes.
    ///
    /// NOTE: This is only intended for beacon consensus (after merge).
    fn cfg_and_block_env(&self, chain_spec: &ChainSpec, parent: &Header) -> (CfgEnv, BlockEnv) {
        // TODO: should be different once revm has configurable cfgenv
        // configure evm env based on parent block
        let mut cfg = CfgEnv::default();
        cfg.chain_id = chain_spec.chain().id();

        #[cfg(feature = "optimism")]
        {
            cfg.optimism = chain_spec.is_optimism();
        }

        // ensure we're not missing any timestamp based hardforks
        cfg.spec_id = revm_spec_by_timestamp_after_merge(chain_spec, self.timestamp());

        // if the parent block did not have excess blob gas (i.e. it was pre-cancun), but it is
        // cancun now, we need to set the excess blob gas to the default value
        let blob_excess_gas_and_price = parent
            .next_block_excess_blob_gas()
            .map_or_else(
                || {
                    if cfg.spec_id == SpecId::CANCUN {
                        // default excess blob gas is zero
                        Some(0)
                    } else {
                        None
                    }
                },
                Some,
            )
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
                    .next_block_base_fee(chain_spec.base_fee_params(self.timestamp()))
                    .unwrap_or_default(),
            ),
            // calculate excess gas based on parent block's blob gas usage
            blob_excess_gas_and_price,
        };

        (cfg, block_env)
    }
}

/// The execution payload attribute type the CL node emits via the engine API.
/// This trait should be implemented by types that could be used to spawn a payload job.
///
/// This type is emitted as part of the forkchoiceUpdated call
pub trait PayloadAttributes:
    serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug + Clone + Send + Sync + 'static
{
    /// Returns the timestamp to be used in the payload job.
    fn timestamp(&self) -> u64;

    /// Returns the withdrawals for the given payload attributes.
    fn withdrawals(&self) -> Option<&Vec<Withdrawal>>;

    /// Return the parent beacon block root for the payload attributes.
    fn parent_beacon_block_root(&self) -> Option<B256>;

    /// Ensures that the payload attributes are valid for the given [ChainSpec] and
    /// [EngineApiMessageVersion].
    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), AttributesValidationError>;
}

impl PayloadAttributes for EthPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.parent_beacon_block_root
    }

    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), AttributesValidationError> {
        validate_version_specific_fields(chain_spec, version, self.into())
    }
}

impl PayloadAttributes for OptimismPayloadAttributes {
    fn timestamp(&self) -> u64 {
        self.payload_attributes.timestamp
    }

    fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
        self.payload_attributes.withdrawals.as_ref()
    }

    fn parent_beacon_block_root(&self) -> Option<B256> {
        self.payload_attributes.parent_beacon_block_root
    }

    fn ensure_well_formed_attributes(
        &self,
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
    ) -> Result<(), AttributesValidationError> {
        validate_version_specific_fields(chain_spec, version, self.into())?;

        if self.gas_limit.is_none() && chain_spec.is_optimism() {
            return Err(AttributesValidationError::InvalidParams(
                "MissingGasLimitInPayloadAttributes".to_string().into(),
            ))
        }

        Ok(())
    }
}
