//! This crate defines abstractions to create and update payloads (blocks)

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod error;

pub use error::{EngineObjectValidationError, PayloadBuilderError, VersionSpecificValidationError};

mod events;
pub use crate::events::{Events, PayloadEvents};

/// Contains traits to abstract over payload attributes types and default implementations of the
/// [`PayloadAttributes`] trait for ethereum mainnet and optimism types.
mod traits;
pub use traits::{
    BuiltPayload, PayloadAttributes, PayloadAttributesBuilder, PayloadBuilder,
    PayloadBuilderAttributes,
};

mod payload;
pub use payload::PayloadOrAttributes;

use reth_chainspec::{ChainSpec, EthereumHardforks};
/// The types that are used by the engine API.
pub trait PayloadTypes: Send + Sync + Unpin + core::fmt::Debug + Clone + 'static {
    /// The built payload type.
    type BuiltPayload: BuiltPayload + Clone + Unpin;

    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributes + Unpin;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributes<RpcPayloadAttributes = Self::PayloadAttributes>
        + Clone
        + Unpin;
}

/// Validates the timestamp depending on the version called:
///
/// * If V2, this ensures that the payload timestamp is pre-Cancun.
/// * If V3, this ensures that the payload timestamp is within the Cancun timestamp.
/// * If V4, this ensures that the payload timestamp is within the Prague timestamp.
///
/// Otherwise, this will return [`EngineObjectValidationError::UnsupportedFork`].
pub fn validate_payload_timestamp(
    chain_spec: impl EthereumHardforks,
    version: EngineApiMessageVersion,
    timestamp: u64,
) -> Result<(), EngineObjectValidationError> {
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
        return Err(EngineObjectValidationError::UnsupportedFork)
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
        return Err(EngineObjectValidationError::UnsupportedFork)
    }

    let is_prague = chain_spec.is_prague_active_at_timestamp(timestamp);
    if version == EngineApiMessageVersion::V4 && !is_prague {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/7907424db935b93c2fe6a3c0faab943adebe8557/src/engine/prague.md#specification-1>
        //
        // For `engine_getPayloadV4`:
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the built payload does not fall within the time frame of the Prague fork.
        //
        // For `engine_forkchoiceUpdatedV4`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
        //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within
        //    the time frame of the Prague fork.
        //
        // For `engine_newPayloadV4`:
        //
        // 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the payload does not fall within the time frame of the Prague fork.
        return Err(EngineObjectValidationError::UnsupportedFork)
    }
    Ok(())
}

/// Validates the presence of the `withdrawals` field according to the payload timestamp.
/// After Shanghai, withdrawals field must be [Some].
/// Before Shanghai, withdrawals field must be [None];
pub fn validate_withdrawals_presence(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    message_validation_kind: MessageValidationKind,
    timestamp: u64,
    has_withdrawals: bool,
) -> Result<(), EngineObjectValidationError> {
    let is_shanghai_active = chain_spec.is_shanghai_active_at_timestamp(timestamp);

    match version {
        EngineApiMessageVersion::V1 => {
            if has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::WithdrawalsNotSupportedInV1))
            }
        }
        EngineApiMessageVersion::V2 | EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 => {
            if is_shanghai_active && !has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::NoWithdrawalsPostShanghai))
            }
            if !is_shanghai_active && has_withdrawals {
                return Err(message_validation_kind
                    .to_error(VersionSpecificValidationError::HasWithdrawalsPreShanghai))
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
/// return [`EngineObjectValidationError::UnsupportedFork`].
///
/// If the timestamp is before the Cancun fork and the engine API message version is V3, then this
/// will return [`EngineObjectValidationError::UnsupportedFork`].
///
/// If the engine API message version is V3, but the `parentBeaconBlockRoot` is [None], then
/// this will return [`VersionSpecificValidationError::NoParentBeaconBlockRootPostCancun`].
///
/// This implements the following Engine API spec rules:
///
/// 1. Client software **MUST** check that provided set of parameters and their fields strictly
///    matches the expected one and return `-32602: Invalid params` error if this check fails. Any
///    field having `null` value **MUST** be considered as not provided.
///
/// For `engine_forkchoiceUpdatedV3`:
///
/// 1. Client software **MUST** check that provided set of parameters and their fields strictly
///    matches the expected one and return `-32602: Invalid params` error if this check fails. Any
///    field having `null` value **MUST** be considered as not provided.
///
/// 2. Extend point (7) of the `engine_forkchoiceUpdatedV1` specification by defining the following
///    sequence of checks that **MUST** be run over `payloadAttributes`:
///     1. `payloadAttributes` matches the `PayloadAttributesV3` structure, return `-38003: Invalid
///        payload attributes` on failure.
///     2. `payloadAttributes.timestamp` falls within the time frame of the Cancun fork, return
///        `-38005: Unsupported fork` on failure.
///     3. `payloadAttributes.timestamp` is greater than `timestamp` of a block referenced by
///        `forkchoiceState.headBlockHash`, return `-38003: Invalid payload attributes` on failure.
///     4. If any of the above checks fails, the `forkchoiceState` update **MUST NOT** be rolled
///        back.
///
/// For `engine_newPayloadV3`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
///    payload does not fall within the time frame of the Cancun fork.
///
/// For `engine_newPayloadV4`:
///
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of the
///    payload does not fall within the time frame of the Prague fork.
///
/// Returning the right error code (ie, if the client should return `-38003: Invalid payload
/// attributes` is handled by the `message_validation_kind` parameter. If the parameter is
/// `MessageValidationKind::Payload`, then the error code will be `-32602: Invalid params`. If the
/// parameter is `MessageValidationKind::PayloadAttributes`, then the error code will be `-38003:
/// Invalid payload attributes`.
pub fn validate_parent_beacon_block_root_presence(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    validation_kind: MessageValidationKind,
    timestamp: u64,
    has_parent_beacon_block_root: bool,
) -> Result<(), EngineObjectValidationError> {
    // 1. Client software **MUST** check that provided set of parameters and their fields strictly
    //    matches the expected one and return `-32602: Invalid params` error if this check fails.
    //    Any field having `null` value **MUST** be considered as not provided.
    //
    // For `engine_forkchoiceUpdatedV3`:
    //
    // 2. Extend point (7) of the `engine_forkchoiceUpdatedV1` specification by defining the
    //    following sequence of checks that **MUST** be run over `payloadAttributes`:
    //     1. `payloadAttributes` matches the `PayloadAttributesV3` structure, return `-38003:
    //        Invalid payload attributes` on failure.
    //     2. `payloadAttributes.timestamp` falls within the time frame of the Cancun fork, return
    //        `-38005: Unsupported fork` on failure.
    //     3. `payloadAttributes.timestamp` is greater than `timestamp` of a block referenced by
    //        `forkchoiceState.headBlockHash`, return `-38003: Invalid payload attributes` on
    //        failure.
    //     4. If any of the above checks fails, the `forkchoiceState` update **MUST NOT** be rolled
    //        back.
    match version {
        EngineApiMessageVersion::V1 | EngineApiMessageVersion::V2 => {
            if has_parent_beacon_block_root {
                return Err(validation_kind.to_error(
                    VersionSpecificValidationError::ParentBeaconBlockRootNotSupportedBeforeV3,
                ))
            }
        }
        EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 => {
            if !has_parent_beacon_block_root {
                return Err(validation_kind
                    .to_error(VersionSpecificValidationError::NoParentBeaconBlockRootPostCancun))
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

/// A type that represents whether or not we are validating a payload or payload attributes.
///
/// This is used to ensure that the correct error code is returned when validating the payload or
/// payload attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageValidationKind {
    /// We are validating fields of a payload attributes.
    PayloadAttributes,
    /// We are validating fields of a payload.
    Payload,
}

impl MessageValidationKind {
    /// Returns an `EngineObjectValidationError` based on the given
    /// `VersionSpecificValidationError` and the current validation kind.
    pub const fn to_error(
        self,
        error: VersionSpecificValidationError,
    ) -> EngineObjectValidationError {
        match self {
            Self::Payload => EngineObjectValidationError::Payload(error),
            Self::PayloadAttributes => EngineObjectValidationError::PayloadAttributes(error),
        }
    }
}

/// Validates the presence or exclusion of fork-specific fields based on the ethereum execution
/// payload, or payload attributes, and the message version.
///
/// The object being validated is provided by the [`PayloadOrAttributes`] argument, which can be
/// either an execution payload, or payload attributes.
///
/// The version is provided by the [`EngineApiMessageVersion`] argument.
pub fn validate_version_specific_fields<Type>(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    payload_or_attrs: PayloadOrAttributes<'_, Type>,
) -> Result<(), EngineObjectValidationError>
where
    Type: PayloadAttributes,
{
    validate_withdrawals_presence(
        chain_spec,
        version,
        payload_or_attrs.message_validation_kind(),
        payload_or_attrs.timestamp(),
        payload_or_attrs.withdrawals().is_some(),
    )?;
    validate_parent_beacon_block_root_presence(
        chain_spec,
        version,
        payload_or_attrs.message_validation_kind(),
        payload_or_attrs.timestamp(),
        payload_or_attrs.parent_beacon_block_root().is_some(),
    )
}

/// The version of Engine API message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EngineApiMessageVersion {
    /// Version 1
    V1,
    /// Version 2
    ///
    /// Added in the Shanghai hardfork.
    V2,
    /// Version 3
    ///
    /// Added in the Cancun hardfork.
    V3,
    /// Version 4
    ///
    /// Added in the Prague hardfork.
    V4,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ord() {
        assert!(EngineApiMessageVersion::V4 > EngineApiMessageVersion::V3);
    }
}
