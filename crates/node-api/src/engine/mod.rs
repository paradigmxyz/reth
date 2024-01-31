//! This contains the [EngineTypes] trait and implementations for ethereum mainnet types.
//!
//! The [EngineTypes] trait can be implemented to configure the engine to work with custom types,
//! as long as those types implement certain traits.
//!
//! Custom payload attributes can be supported by implementing two main traits:
//!
//! [PayloadAttributes] can be implemented for payload attributes types that are used as
//! arguments to the `engine_forkchoiceUpdated` method. This type should be used to define and
//! _spawn_ payload jobs.
//!
//! [PayloadBuilderAttributes] can be implemented for payload attributes types that _describe_
//! running payload jobs.
//!
//! Once traits are implemented and custom types are defined, the [EngineTypes] trait can be
//! implemented:
//! ```no_run
//! # use reth_rpc_types::engine::{PayloadAttributes as EthPayloadAttributes, PayloadId};
//! # use reth_rpc_types::Withdrawal;
//! # use reth_primitives::{B256, ChainSpec, Address};
//! # use reth_node_api::{EngineTypes, EngineApiMessageVersion, validate_version_specific_fields, AttributesValidationError, PayloadAttributes, PayloadBuilderAttributes, PayloadOrAttributes};
//! # use reth_payload_builder::{EthPayloadBuilderAttributes, EthBuiltPayload};
//! # use serde::{Deserialize, Serialize};
//! # use thiserror::Error;
//! # use std::convert::Infallible;
//!
//! /// A custom payload attributes type.
//! #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
//! pub struct CustomPayloadAttributes {
//!     /// An inner payload type
//!     #[serde(flatten)]
//!     pub inner: EthPayloadAttributes,
//!     /// A custom field
//!     pub custom: u64,
//! }
//!
//! /// Custom error type used in payload attributes validation
//! #[derive(Debug, Error)]
//! pub enum CustomError {
//!    #[error("Custom field is not zero")]
//!    CustomFieldIsNotZero,
//! }
//!
//! impl PayloadAttributes for CustomPayloadAttributes {
//!     fn timestamp(&self) -> u64 {
//!         self.inner.timestamp()
//!     }
//!
//!     fn withdrawals(&self) -> Option<&Vec<Withdrawal>> {
//!         self.inner.withdrawals()
//!     }
//!
//!     fn parent_beacon_block_root(&self) -> Option<B256> {
//!         self.inner.parent_beacon_block_root()
//!     }
//!
//!     fn ensure_well_formed_attributes(
//!         &self,
//!         chain_spec: &ChainSpec,
//!         version: EngineApiMessageVersion,
//!     ) -> Result<(), AttributesValidationError> {
//!         validate_version_specific_fields(chain_spec, version, self.into())?;
//!
//!         // custom validation logic - ensure that the custom field is not zero
//!         if self.custom == 0 {
//!             return Err(AttributesValidationError::invalid_params(
//!                 CustomError::CustomFieldIsNotZero,
//!             ))
//!         }
//!
//!         Ok(())
//!     }
//! }
//!
//! /// Newtype around the payload builder attributes type
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub struct CustomPayloadBuilderAttributes(EthPayloadBuilderAttributes);
//!
//! impl PayloadBuilderAttributes for CustomPayloadBuilderAttributes {
//!     type RpcPayloadAttributes = CustomPayloadAttributes;
//!     type Error = Infallible;
//!
//!     fn try_new(parent: B256, attributes: CustomPayloadAttributes) -> Result<Self, Infallible> {
//!         Ok(Self(EthPayloadBuilderAttributes::new(parent, attributes.inner)))
//!     }
//!
//!     fn parent(&self) -> B256 {
//!         self.0.parent
//!     }
//!
//!     fn payload_id(&self) -> PayloadId {
//!         self.0.id
//!     }
//!
//!     fn timestamp(&self) -> u64 {
//!         self.0.timestamp
//!     }
//!
//!     fn parent_beacon_block_root(&self) -> Option<B256> {
//!         self.0.parent_beacon_block_root
//!     }
//!
//!     fn suggested_fee_recipient(&self) -> Address {
//!         self.0.suggested_fee_recipient
//!     }
//!
//!     fn prev_randao(&self) -> B256 {
//!         self.0.prev_randao
//!     }
//!
//!     fn withdrawals(&self) -> &Vec<reth_primitives::Withdrawal> {
//!         &self.0.withdrawals
//!     }
//! }
//!
//! /// Custom engine types - uses a custom payload attributes RPC type, but uses the default
//! /// payload builder attributes type.
//! #[derive(Clone, Debug, Default)]
//! #[non_exhaustive]
//! pub struct CustomEngineTypes;
//!
//! impl EngineTypes for CustomEngineTypes {
//!    type PayloadAttributes = CustomPayloadAttributes;
//!    type PayloadBuilderAttributes = CustomPayloadBuilderAttributes;
//!    type BuiltPayload = EthBuiltPayload;
//!
//!    fn validate_version_specific_fields(
//!        chain_spec: &ChainSpec,
//!        version: EngineApiMessageVersion,
//!        payload_or_attrs: PayloadOrAttributes<'_, CustomPayloadAttributes>,
//!    ) -> Result<(), AttributesValidationError> {
//!        validate_version_specific_fields(chain_spec, version, payload_or_attrs)
//!    }
//! }
//! ```

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

/// The types that are used by the engine.
pub trait EngineTypes: Send + Sync {
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
        //    payload or payloadAttributes greater or equal to the Cancun activation timestamp.
        return Err(AttributesValidationError::UnsupportedFork)
    }

    if version == EngineApiMessageVersion::V3 && !is_cancun {
        // From the Engine API spec:
        // <https://github.com/ethereum/execution-apis/blob/ff43500e653abde45aec0f545564abfb648317af/src/engine/cancun.md#specification-2>
        //
        // 1. Client software **MUST** return `-38005: Unsupported fork` error if the `timestamp` of
        //    the built payload does not fall within the time frame of the Cancun fork.
        return Err(AttributesValidationError::UnsupportedFork)
    }
    Ok(())
}

#[cfg(feature = "optimism")]
/// Validates the presence of the `withdrawals` field according to the payload timestamp.
///
/// After Canyon, withdrawals field must be [Some].
/// Before Canyon, withdrawals field must be [None];
///
/// Canyon activates the Shanghai EIPs, see the Canyon specs for more details:
/// <https://github.com/ethereum-optimism/optimism/blob/ab926c5fd1e55b5c864341c44842d6d1ca679d99/specs/superchain-upgrades.md#canyon>
pub fn optimism_validate_withdrawals_presence(
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

/// Validate the presence of the `parentBeaconBlockRoot` field according to the payload
/// timestamp.
///
/// After Cancun, `parentBeaconBlockRoot` field must be [Some].
/// Before Cancun, `parentBeaconBlockRoot` field must be [None].
///
/// If the engine API message version is V1 or V2, and the payload attribute's timestamp is
/// post-Cancun, then this will return [AttributesValidationError::UnsupportedFork].
///
/// If the payload attribute's timestamp is before the Cancun fork and the engine API message
/// version is V3, then this will return [AttributesValidationError::UnsupportedFork].
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
/// 2. Client software **MUST** return `-38005: Unsupported fork` error if the `payloadAttributes`
///    is set and the `payloadAttributes.timestamp` does not fall within the time frame of the
///    Cancun fork.
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

    // 2. Client software **MUST** return `-38005: Unsupported fork` error if the
    //    `payloadAttributes` is set and the `payloadAttributes.timestamp` does not fall within the
    //    time frame of the Cancun fork.
    validate_payload_timestamp(chain_spec, version, timestamp)?;

    Ok(())
}

/// Validates the presence or exclusion of fork-specific fields based on the payload attributes
/// and the message version.
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

#[cfg(feature = "optimism")]
/// Validates the presence or exclusion of fork-specific fields based on the payload attributes
/// and the message version.
pub fn optimism_validate_version_specific_fields<Type>(
    chain_spec: &ChainSpec,
    version: EngineApiMessageVersion,
    payload_or_attrs: PayloadOrAttributes<'_, Type>,
) -> Result<(), AttributesValidationError>
where
    Type: PayloadAttributes,
{
    optimism_validate_withdrawals_presence(
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
