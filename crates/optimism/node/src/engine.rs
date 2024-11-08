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
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::{OptimismHardfork, OptimismHardforks};
use reth_optimism_payload_builder::{OpBuiltPayload, OpPayloadBuilderAttributes};

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct OpEngineTypes<T: PayloadTypes = OpPayloadTypes> {
    _marker: std::marker::PhantomData<T>,
}

impl<T: PayloadTypes> PayloadTypes for OpEngineTypes<T> {
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;
}

impl<T: PayloadTypes> EngineTypes for OpEngineTypes<T>
where
    T::BuiltPayload: TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<OpExecutionPayloadEnvelopeV3>
        + TryInto<OpExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = OpExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = OpExecutionPayloadEnvelopeV4;
}

/// A default payload type for [`OpEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct OpPayloadTypes;

impl PayloadTypes for OpPayloadTypes {
    type BuiltPayload = OpBuiltPayload;
    type PayloadAttributes = OpPayloadAttributes;
    type PayloadBuilderAttributes = OpPayloadBuilderAttributes;
}

/// Validator for Optimism engine API.
#[derive(Debug, Clone)]
pub struct OpEngineValidator {
    chain_spec: Arc<OpChainSpec>,
}

impl OpEngineValidator {
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

impl<Types> EngineValidator<Types> for OpEngineValidator
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

        if self.chain_spec.is_holocene_active_at_timestamp(attributes.payload_attributes.timestamp)
        {
            let (elasticity, denominator) =
                attributes.decode_eip_1559_params().ok_or_else(|| {
                    EngineObjectValidationError::InvalidParams(
                        "MissingEip1559ParamsInPayloadAttributes".to_string().into(),
                    )
                })?;
            if elasticity != 0 && denominator == 0 {
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

    use crate::engine;
    use alloy_primitives::{b64, Address, B256, B64};
    use alloy_rpc_types_engine::PayloadAttributes;
    use reth_chainspec::ForkCondition;
    use reth_optimism_chainspec::BASE_SEPOLIA;

    use super::*;

    fn get_chainspec(is_holocene: bool) -> Arc<OpChainSpec> {
        let mut hardforks = OptimismHardfork::base_sepolia();
        if is_holocene {
            hardforks
                .insert(OptimismHardfork::Holocene.boxed(), ForkCondition::Timestamp(1800000000));
        }
        Arc::new(OpChainSpec {
            inner: ChainSpec {
                chain: BASE_SEPOLIA.inner.chain,
                genesis: BASE_SEPOLIA.inner.genesis.clone(),
                genesis_hash: BASE_SEPOLIA.inner.genesis_hash.clone(),
                paris_block_and_final_difficulty: BASE_SEPOLIA
                    .inner
                    .paris_block_and_final_difficulty,
                hardforks,
                base_fee_params: BASE_SEPOLIA.inner.base_fee_params.clone(),
                max_gas_limit: BASE_SEPOLIA.inner.max_gas_limit,
                prune_delete_limit: 10000,
                ..Default::default()
            },
        })
    }

    const fn get_attributes(eip_1559_params: Option<B64>, timestamp: u64) -> OpPayloadAttributes {
        OpPayloadAttributes {
            gas_limit: Some(1000),
            eip_1559_params,
            transactions: None,
            no_tx_pool: None,
            payload_attributes: PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            },
        }
    }

    #[test]
    fn test_well_formed_attributes_pre_holocene() {
        let validator = OpEngineValidator::new(get_chainspec(false));
        let attributes = get_attributes(None, 1799999999);

        let result = <engine::OpEngineValidator as reth_node_builder::EngineValidator<
            OpEngineTypes,
        >>::ensure_well_formed_attributes(
            &validator, EngineApiMessageVersion::V3, &attributes
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_well_formed_attributes_holocene_no_eip1559_params() {
        let validator = OpEngineValidator::new(get_chainspec(true));
        let attributes = get_attributes(None, 1800000000);

        let result = <engine::OpEngineValidator as reth_node_builder::EngineValidator<
            OpEngineTypes,
        >>::ensure_well_formed_attributes(
            &validator, EngineApiMessageVersion::V3, &attributes
        );
        assert!(matches!(result, Err(EngineObjectValidationError::InvalidParams(_))));
    }

    #[test]
    fn test_well_formed_attributes_holocene_eip1559_params_zero_denominator() {
        let validator = OpEngineValidator::new(get_chainspec(true));
        let attributes = get_attributes(Some(b64!("0000000000000008")), 1800000000);

        let result = <engine::OpEngineValidator as reth_node_builder::EngineValidator<
            OpEngineTypes,
        >>::ensure_well_formed_attributes(
            &validator, EngineApiMessageVersion::V3, &attributes
        );
        assert!(matches!(result, Err(EngineObjectValidationError::InvalidParams(_))));
    }

    #[test]
    fn test_well_formed_attributes_holocene_valid() {
        let validator = OpEngineValidator::new(get_chainspec(true));
        let attributes = get_attributes(Some(b64!("0000000800000008")), 1800000000);

        let result = <engine::OpEngineValidator as reth_node_builder::EngineValidator<
            OpEngineTypes,
        >>::ensure_well_formed_attributes(
            &validator, EngineApiMessageVersion::V3, &attributes
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_well_formed_attributes_holocene_valid_all_zero() {
        let validator = OpEngineValidator::new(get_chainspec(true));
        let attributes = get_attributes(Some(b64!("0000000000000000")), 1800000000);

        let result = <engine::OpEngineValidator as reth_node_builder::EngineValidator<
            OpEngineTypes,
        >>::ensure_well_formed_attributes(
            &validator, EngineApiMessageVersion::V3, &attributes
        );
        assert!(result.is_ok());
    }
}
