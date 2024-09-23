//! Traits, validation methods, and helper types used to abstract over engine types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod invalid_block_hook;
pub use invalid_block_hook::InvalidBlockHook;

pub use reth_payload_primitives::{
    BuiltPayload, EngineApiMessageVersion, EngineObjectValidationError, PayloadOrAttributes,
    PayloadTypes,
};
use serde::{de::DeserializeOwned, ser::Serialize};

/// This type defines the versioned types of the engine API.
///
/// This includes the execution payload types and payload attributes that are used to trigger a
/// payload job. Hence this trait is also [`PayloadTypes`].
pub trait EngineTypes:
    PayloadTypes<
        BuiltPayload: TryInto<Self::ExecutionPayloadV1>
                          + TryInto<Self::ExecutionPayloadV2>
                          + TryInto<Self::ExecutionPayloadV3>
                          + TryInto<Self::ExecutionPayloadV4>,
    > + DeserializeOwned
    + Serialize
    + 'static
{
    /// The chain specification of the node.
    type ChainSpec: Send + Sync;

    /// Execution Payload V1 type.
    type ExecutionPayloadV1: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;
    /// Execution Payload V2 type.
    type ExecutionPayloadV2: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;
    /// Execution Payload V3 type.
    type ExecutionPayloadV3: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;
    /// Execution Payload V4 type.
    type ExecutionPayloadV4: DeserializeOwned + Serialize + Clone + Unpin + Send + Sync + 'static;

    /// Validates the presence or exclusion of fork-specific fields based on the payload attributes
    /// and the message version.
    fn validate_version_specific_fields(
        chain_spec: &Self::ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, Self::PayloadAttributes>,
    ) -> Result<(), EngineObjectValidationError>;
}
