use std::sync::Arc;

use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1};
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpPayloadAttributes,
};
use reth_chainspec::ChainSpec;
use reth_node_api::{
    payload::{
        validate_parent_beacon_block_root_presence, EngineApiMessageVersion,
        EngineObjectValidationError, MessageValidationKind, PayloadOrAttributes, PayloadTypes,
        VersionSpecificValidationError,
    },
    validate_version_specific_fields, EngineTypes, EngineValidator,
};
use reth_optimism_chainspec::{decode_holocene_1559_params, OpChainSpec};
use reth_optimism_forks::OptimismHardfork;
use reth_optimism_payload_builder::{OptimismBuiltPayload, OptimismPayloadBuilderAttributes};

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct OptimismEngineTypes<T: PayloadTypes = OptimismPayloadTypes> {
    _marker: std::marker::PhantomData<T>,
}

impl<T: PayloadTypes> PayloadTypes for OptimismEngineTypes<T> {
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;
}

impl<T: PayloadTypes> EngineTypes for OptimismEngineTypes<T>
where
    T::BuiltPayload: TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<OpExecutionPayloadEnvelopeV3>
        + TryInto<OpExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadV1 = ExecutionPayloadV1;
    type ExecutionPayloadV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadV4 = OpExecutionPayloadEnvelopeV4;
}

/// A default payload type for [`OptimismEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct OptimismPayloadTypes;

impl PayloadTypes for OptimismPayloadTypes {
    type BuiltPayload = OptimismBuiltPayload;
    type PayloadAttributes = OpPayloadAttributes;
    type PayloadBuilderAttributes = OptimismPayloadBuilderAttributes;
}

/// Validator for Optimism engine API.
#[derive(Debug, Clone)]
pub struct OptimismEngineValidator {
    chain_spec: Arc<OpChainSpec>,
}

impl OptimismEngineValidator {
    /// Instantiates a new validator.
    pub const fn new(chain_spec: Arc<OpChainSpec>) -> Self {
        Self { chain_spec }
    }
}

/// Validates the presence of the `withdrawals` field according to the payload timestamp.
///
/// After Canyon, withdrawals field must be [Some].
/// Before Canyon, withdrawals field must be [None];
///
/// Canyon activates the Shanghai EIPs, see the Canyon specs for more details:
/// <https://github.com/ethereum-optimism/optimism/blob/ab926c5fd1e55b5c864341c44842d6d1ca679d99/specs/superchain-upgrades.md#canyon>
pub fn validate_withdrawals_presence(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    message_validation_kind: MessageValidationKind,
    timestamp: u64,
    has_withdrawals: bool,
) -> Result<(), EngineObjectValidationError> {
    let is_shanghai = chain_spec.fork(OptimismHardfork::Canyon).active_at_timestamp(timestamp);

    match version {
        EngineApiMessageVersion::V1 => {
            if has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::WithdrawalsNotSupportedInV1))
            }
            if is_shanghai {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::NoWithdrawalsPostShanghai))
            }
        }
        EngineApiMessageVersion::V2 | EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 => {
            if is_shanghai && !has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::NoWithdrawalsPostShanghai))
            }
            if !is_shanghai && has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::HasWithdrawalsPreShanghai))
            }
        }
    };

    Ok(())
}

impl<Types> EngineValidator<Types> for OptimismEngineValidator
where
    Types: EngineTypes<PayloadAttributes = OpPayloadAttributes>,
{
    fn validate_version_specific_fields(
        &self,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, OpPayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_withdrawals_presence(
            &self.chain_spec,
            version,
            payload_or_attrs.message_validation_kind(),
            payload_or_attrs.timestamp(),
            payload_or_attrs.withdrawals().is_some(),
        )?;
        validate_parent_beacon_block_root_presence(
            &self.chain_spec,
            version,
            payload_or_attrs.message_validation_kind(),
            payload_or_attrs.timestamp(),
            payload_or_attrs.parent_beacon_block_root().is_some(),
        )
    }

    fn ensure_well_formed_attributes(
        &self,
        version: EngineApiMessageVersion,
        attributes: &OpPayloadAttributes,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(&self.chain_spec, version, attributes.into())?;

        if attributes.gas_limit.is_none() {
            return Err(EngineObjectValidationError::InvalidParams(
                "MissingGasLimitInPayloadAttributes".to_string().into(),
            ))
        }
        
        println!("well_formed attributes: {:?}", attributes);

        if self.chain_spec.is_fork_active_at_timestamp(
            OptimismHardfork::Holocene,
            attributes.payload_attributes.timestamp,
        ) {
            if attributes.eip_1559_params.is_none() {
                return Err(EngineObjectValidationError::InvalidParams(
                    "MissingEip1559ParamsInPayloadAttributes".to_string().into(),
                ))
            }

            let (_, denominator) = decode_holocene_1559_params(
                attributes.eip_1559_params.expect("eip_1559_params is none"),
            );

            if denominator == 0 {
                return Err(EngineObjectValidationError::InvalidParams(
                    "Eip1559ParamsDenominatorZero".to_string().into(),
                ))
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {

    use alloy_primitives::{Address, B256};
    use alloy_rpc_types_engine::PayloadAttributes;
    use reth_chainspec::ForkCondition;
    use reth_optimism_chainspec::BASE_SEPOLIA;
    use crate::engine;

    use super::*;

    fn pre_holocene_chainspec() -> Arc<OpChainSpec> {
        let hardforks = OptimismHardfork::base_sepolia();
        Arc::new(OpChainSpec {
            inner: ChainSpec {
                chain: BASE_SEPOLIA.inner.chain,
                genesis: BASE_SEPOLIA.inner.genesis.clone(),
                genesis_hash: BASE_SEPOLIA.inner.genesis_hash.clone(),
                paris_block_and_final_difficulty: BASE_SEPOLIA.inner.paris_block_and_final_difficulty,
                hardforks,
                base_fee_params: BASE_SEPOLIA.inner.base_fee_params.clone(),
                max_gas_limit: BASE_SEPOLIA.inner.max_gas_limit,
                prune_delete_limit: 10000,
                ..Default::default()
            },
        })
    }


    fn holocene_chainspec() -> Arc<OpChainSpec> {
        let mut hardforks = OptimismHardfork::base_sepolia();
        hardforks.insert(OptimismHardfork::Holocene.boxed(), ForkCondition::Timestamp(1800000000));
        Arc::new(OpChainSpec {
            inner: ChainSpec {
                chain: BASE_SEPOLIA.inner.chain,
                genesis: BASE_SEPOLIA.inner.genesis.clone(),
                genesis_hash: BASE_SEPOLIA.inner.genesis_hash.clone(),
                paris_block_and_final_difficulty: BASE_SEPOLIA.inner.paris_block_and_final_difficulty,
                hardforks,
                base_fee_params: BASE_SEPOLIA.inner.base_fee_params.clone(),
                max_gas_limit: BASE_SEPOLIA.inner.max_gas_limit,
                prune_delete_limit: 10000,
                ..Default::default()
            },
        })
    }

    #[test]
    fn test_well_formed_attributes_pre_holocene() {
        let validator = OptimismEngineValidator::new(pre_holocene_chainspec());

        let attributes = OpPayloadAttributes {
            gas_limit: Some(1000),
            eip_1559_params: None,
            transactions: None,
            no_tx_pool: None,
            payload_attributes: PayloadAttributes {
                timestamp: 0x1337,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Default::default(),
                parent_beacon_block_root: Some(B256::ZERO),
            },
        };

        // assert!(<engine::OptimismEngineValidator as reth_node_builder::EngineValidator<EngineTypes<PayloadAttributes = OpPayloadAttributes>>>::ensure_well_formed_attributes(&validator, EngineApiMessageVersion::V4, &attributes).is_ok());
        // assert!(validator.ensure_well_formed_attributes(EngineApiMessageVersion::V4, &attributes).is_ok());
    }
}
