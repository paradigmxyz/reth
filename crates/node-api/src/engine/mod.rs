//! This contains the [EngineTypes] trait and implementations for ethereum mainnet types.

use core::fmt;
use reth_primitives::{ChainSpec, Hardfork};

/// Contains traits to abstract over payload attributes types and default implementations of the
/// [PayloadAttributes] trait for ethereum mainnet and optimism types.
pub mod traits;
pub use traits::{BuiltPayload, PayloadAttributes, PayloadBuilderAttributes};

/// Contains error types used in the traits defined in this crate.
pub mod error;
pub use error::AttributesValidationError;

/// Contains types used in implementations of the [PayloadAttributes] trait.
pub mod payload;
pub use payload::PayloadOrAttributes;

/// The types that are used by the engine API.
pub trait EngineTypes:
    serde::de::DeserializeOwned + fmt::Debug + Unpin + Send + Sync + Clone
{
    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributes + Unpin;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributes<RpcPayloadAttributes = Self::PayloadAttributes>
        + Clone
        + Unpin;

    /// The built payload type.
    type BuiltPayload: BuiltPayload + Clone + Unpin;

    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Self::PayloadAttributes>,
    ) -> Result<(), AttributesValidationError>;
}

/// Validates the timestamp depending on the version called:
///
/// * If V2, this ensure that the payload timestamp is pre-Cancun.
/// * If V3, this ensures that the payload timestamp is within the Cancun timestamp.
///
/// Otherwise, this will return [AttributesValidationError::UnsupportedFork].
pub fn validate_payload_timestamp(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    timestamp: u64,
) -> Result<(), AttributesValidationError> {
    let is_cancun = chain_spec.is_cancun_active_at_timestamp(timestamp);
    if version == EngineApiMessageVersion::V2 && is_cancun {
        // From the Engine API spec:
        //
        // ### Update the methods of previous forks
        //
        // This document defines how Cancun payload should be handled by the [`Shanghai
        // API`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md).
        //
        // For the following methods:
        //
        // - [`engine_forkchoiceUpdatedV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_forkchoiceupdatedv2)
        // - [`engine_newPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_newpayloadV2)
        // - [`engine_getPayloadV2`](https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/shanghai.md#engine_getpayloadv2)
        //
        // a validation **MUST** be added:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    payload or payloadAttributes is greater or equal to the Cancun activation timestamp.
        return Err(AttributesValidationError::UnsupportedFork)
    }

    if version == EngineApiMessageVersion::V3 && !is_cancun {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/cancun.md#specification-2>
        //
        // For `engine_getPayloadV3`:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the built payload does not fall within the time frame of the Cancun fork.
        //
        // For `engine_forkchoiceUpdatedV3`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
        //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within
        //    the time frame of the Cancun fork.
        //
        // For `engine_newPayloadV3`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the payload does not fall within the time frame of the Cancun fork.
        return Err(AttributesValidationError::UnsupportedFork)
    }
    Ok(())
}

/// Validates the presence of the `withdrawals` field according to the payload timestamp.
/// After Shanghai, withdrawals field must be [Some].
/// Before Shanghai, withdrawals field must be [None];
pub fn validate_withdrawals_presence(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    timestamp: u64,
    has_withdrawals: bool,
) -> Result<(), AttributesValidationError> {
    let is_shanghai = chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(timestamp);

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

/// Validate the presence of the `parentBeaconBlockRoot` field according to the given timestamp.
/// This method is meant to be used with either a `payloadAttributes` field or a full payload, with
/// the `engine_forkchoiceUpdated` and `engine_newPayload` methods respectively.
///
/// After Cancun, the `parentBeaconBlockRoot` field must be [Some].
/// Before Cancun, the `parentBeaconBlockRoot` field must be [None].
///
/// If the engine API message version is V1 or V2, and the timestamp is post-Cancun, then this will
/// return [AttributesValidationError::UnsupportedFork].
///
/// If the timestamp is before the Cancun fork and the engine API message version is V3, then this
/// will return [AttributesValidationError::UnsupportedFork].
///
/// If the engine API message version is V3, but the `parentBeaconBlockRoot` is [None], then
/// this will return [AttributesValidationError::NoParentBeaconBlockRootPostCancun].
///
/// This implements the following Engine API spec rules:
///
/// 1. Client software **MUST** check that provided set of parameters and their fields strictly
///    matches the expected one and return `-32602: Invalid params` error if this check fails. Any
///    field having `null` value **MUST** be considered as not provided.
///
/// For `engine_forkchoiceUpdatedV3`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `payloadAttributes`
///    is set and the `payloadAttributes.timestamp` does not fall within the time frame of the
///    Cancun fork.
///
/// For `engine_newPayloadV3`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
///    payload does not fall within the time frame of the Cancun fork.
pub fn validate_parent_beacon_block_root_presence(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    timestamp: u64,
    has_parent_beacon_block_root: bool,
) -> Result<(), AttributesValidationError> {
    // 1. Client software **MUST** check that provided set of parameters and their fields strictly
    //    matches the expected one and return `-32602: Invalid params` error if this check fails.
    //    Any field having `null` value **MUST** be considered as not provided.
    match version {
        EngineApiMessageVersion::V1 | EngineApiMessageVersion::V2 => {
            if has_parent_beacon_block_root {
                return Err(AttributesValidationError::ParentBeaconBlockRootNotSupportedBeforeV3)
            }
        }
        EngineApiMessageVersion::V3 => {
            if !has_parent_beacon_block_root {
                return Err(AttributesValidationError::NoParentBeaconBlockRootPostCancun)
            }
        }
    };

    // For `engine_forkchoiceUpdatedV3`:
    //
    // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
    //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within the
    //    time frame of the Cancun fork.
    //
    // For `engine_newPayloadV3`:
    //
    // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
    //    payload does not fall within the time frame of the Cancun fork.
    validate_payload_timestamp(chain_spec, version, timestamp)?;

    Ok(())
}

/// Validates the presence or exclusion of fork-specific fields based on the ethereum payload
/// attributes and the message version.
pub fn validate_version_specific_fields<Type>(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    payload_or_attrs: PayloadOrAttributes<'_, Type>,
) -> Result<(), AttributesValidationError>
where
    Type: PayloadAttributes,
{
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

/// The version of Engine API message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineApiMessageVersion {
    /// Version 1
    V1,
    /// Version 2
    ///
    /// Added for shanghai hardfork.
    V2,
    /// Version 3
    ///
    /// Added for cancun hardfork.
    V3,
}
