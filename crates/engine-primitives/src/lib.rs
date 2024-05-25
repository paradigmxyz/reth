//! Traits, validation methods, and helper types used to abstract over engine types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use core::fmt;
use reth_payload_primitives::{
    error::EngineObjectValidationError, BuiltPayload, EngineApiMessageVersion, PayloadAttributes,
    PayloadBuilderAttributes, PayloadOrAttributes,
};
use reth_primitives::ChainSpec;

/// Contains traits to abstract over payload attributes types and default implementations of
/// the [PayloadAttributes] trait for ethereum mainnet and optimism types.
use serde::{de::DeserializeOwned, ser::Serialize};

/// The types that are used by the engine API.
pub trait EngineTypes:
    serde::de::DeserializeOwned + Serialize + fmt::Debug + Unpin + Send + Sync + Clone
{
    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributes + Unpin;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributes<RpcPayloadAttributes = Self::PayloadAttributes>
        + Clone
        + Unpin;

    /// The built payload type.
    type BuiltPayload: BuiltPayload
        + Clone
        + Unpin
        + TryInto<Self::ExecutionPayloadV1>
        + TryInto<Self::ExecutionPayloadV2>
        + TryInto<Self::ExecutionPayloadV3>;

    /// Execution Payload V1 type.
    type ExecutionPayloadV1: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;
    /// Execution Payload V2 type.
    type ExecutionPayloadV2: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;
    /// Execution Payload V3 type.
    type ExecutionPayloadV3: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;

    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Self::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError>;
}
