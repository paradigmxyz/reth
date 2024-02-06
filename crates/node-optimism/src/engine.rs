use reth_node_api::{
    engine::validate_parent_beacon_block_root_presence, AttributesValidationError,
    EngineApiMessageVersion, EngineTypes, PayloadOrAttributes,
};
use reth_payload_builder::{EthBuiltPayload, OptimismPayloadBuilderAttributes};
use reth_primitives::{ChainSpec, Hardfork};
use reth_rpc_types::engine::OptimismPayloadAttributes;

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize)]
#[non_exhaustive]
pub struct OptimismEngineTypes;

impl EngineTypes for OptimismEngineTypes {
    type PayloadAttributes = OptimismPayloadAttributes;
    type PayloadBuilderAttributes = OptimismPayloadBuilderAttributes;
    type BuiltPayload = EthBuiltPayload;

    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Self::PayloadAttributes>,
    ) -> Result<(), AttributesValidationError> {
        validate_withdrawals_presence(
            chain_spec,
            version,
            payload_or_attrs.timestamp(),
            payload_or_attrs.withdrawals().is_some(),
        )?;
        validate_parent_beacon_block_root_presence(
            chain_spec,
            version,
            payload_or_attrs.timestamp(),
            payload_or_attrs.parent_beacon_block_root().is_some(),
        )
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
    timestamp: u64,
    has_withdrawals: bool,
) -> Result<(), AttributesValidationError> {
    let is_shanghai = chain_spec.fork(Hardfork::Canyon).active_at_timestamp(timestamp);

    match version {
        EngineApiMessageVersion::V1 => {
            if has_withdrawals {
                return Err(AttributesValidationError::WithdrawalsNotSupportedInV1)
            }
            if is_shanghai {
                return Err(AttributesValidationError::NoWithdrawalsPostShanghai)
            }
        }
        EngineApiMessageVersion::V2 | EngineApiMessageVersion::V3 => {
            if is_shanghai && !has_withdrawals {
                return Err(AttributesValidationError::NoWithdrawalsPostShanghai)
            }
            if !is_shanghai && has_withdrawals {
                return Err(AttributesValidationError::HasWithdrawalsPreShanghai)
            }
        }
    };

    Ok(())
}
