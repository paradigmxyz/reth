//! Ethereum specific engine API types and impls.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod payload;
pub use payload::{EthBuiltPayload, EthPayloadBuilderAttributes};

use alloy_rpc_types_engine::{ExecutionData, ExecutionPayload};
pub use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadV1, PayloadAttributes as EthPayloadAttributes,
};
use reth_engine_primitives::EngineTypes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthEngineTypes<T: PayloadTypes = EthPayloadTypes> {
    _marker: core::marker::PhantomData<T>,
}

impl<T: PayloadTypes> PayloadTypes for EthEngineTypes<T> {
    type ExecutionData = T::ExecutionData;
    type BuiltPayload = T::BuiltPayload;
    type PayloadAttributes = T::PayloadAttributes;
    type PayloadBuilderAttributes = T::PayloadBuilderAttributes;
}

impl<T> EngineTypes for EthEngineTypes<T>
where
    T: PayloadTypes<ExecutionData = ExecutionData>,
    T::BuiltPayload: BuiltPayload<Primitives: NodePrimitives<Block = reth_ethereum_primitives::Block>>
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>
        + TryInto<ExecutionPayloadEnvelopeV3>
        + TryInto<ExecutionPayloadEnvelopeV4>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV3;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV4;

    fn block_to_payload(
        block: SealedBlock<
            <<Self::BuiltPayload as BuiltPayload>::Primitives as NodePrimitives>::Block,
        >,
    ) -> T::ExecutionData {
        let (payload, sidecar) =
            ExecutionPayload::from_block_unchecked(block.hash(), &block.into_block());
        ExecutionData { payload, sidecar }
    }
}

/// A default payload type for [`EthEngineTypes`]
#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct EthPayloadTypes;

impl PayloadTypes for EthPayloadTypes {
    type BuiltPayload = EthBuiltPayload;
    type PayloadAttributes = EthPayloadAttributes;
    type PayloadBuilderAttributes = EthPayloadBuilderAttributes;
    type ExecutionData = ExecutionData;
}
