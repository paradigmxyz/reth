//! Ethereum specific

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod payload;
pub use payload::{EthBuiltPayload, EthPayloadBuilderAttributes};
use reth_chainspec::ChainSpec;
use reth_engine_primitives::{BuiltPayload, EngineTypes};
use reth_payload_primitives::{
    validate_version_specific_fields, EngineApiMessageVersion, EngineObjectValidationError,
    PayloadAttributes, PayloadBuilderAttributes, PayloadOrAttributes, PayloadTypes,
};
pub use reth_rpc_types::{
    engine::{
        ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
        PayloadAttributes as EthPayloadAttributes,
    },
    ExecutionPayloadV1,
};
use serde::{de::DeserializeOwned, Serialize};

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthEngineTypes<P, PA, PBA, V1, V2, V3, V4> {
    _phantom: std::marker::PhantomData<(P, PA, PBA, V1, V2, V3, V4)>,
}

/// The default mainnet ethereum beacon consensus engine.
pub type EthEngine = EthEngineTypes<
    EthBuiltPayload,
    EthPayloadAttributes,
    EthPayloadBuilderAttributes,
    ExecutionPayloadV1,
    ExecutionPayloadEnvelopeV2,
    ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4,
>;

impl<P, PA, PBA, V1, V2, V3, V4> PayloadTypes for EthEngineTypes<P, PA, PBA, V1, V2, V3, V4>
where
    Self: std::marker::Unpin,
    P: BuiltPayload + Unpin + Clone,
    PA: PayloadAttributes + Unpin + Clone,
    PBA: PayloadBuilderAttributes<RpcPayloadAttributes = PA> + Unpin + Clone,
    V1: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V2: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V3: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V4: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
{
    type BuiltPayload = P;
    type PayloadAttributes = PA;
    type PayloadBuilderAttributes = PBA;
}

impl<P, PA, PBA, V1, V2, V3, V4> EngineTypes for EthEngineTypes<P, PA, PBA, V1, V2, V3, V4>
where
    Self: std::marker::Unpin,
    P: BuiltPayload + Unpin + Clone,
    PA: PayloadAttributes + Unpin + Clone,
    PBA: PayloadBuilderAttributes<RpcPayloadAttributes = PA> + Unpin + Clone,
    V1: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V2: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V3: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
    V4: DeserializeOwned
        + std::fmt::Debug
        + Serialize
        + Clone
        + Unpin
        + Send
        + Sync
        + 'static
        + std::convert::From<P>,
{
    type ExecutionPayloadV1 = V1;
    type ExecutionPayloadV2 = V2;
    type ExecutionPayloadV3 = V3;
    type ExecutionPayloadV4 = V4;

    fn validate_version_specific_fields(
        chain_spec: &ChainSpec,
        version: EngineApiMessageVersion,
        payload_or_attrs: PayloadOrAttributes<'_, PA>,
    ) -> Result<(), EngineObjectValidationError> {
        validate_version_specific_fields(chain_spec, version, payload_or_attrs)
    }
}
