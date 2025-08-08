#![allow(unused)]

use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV2, ExecutionPayloadV1};
use reth_node_api::payload::PayloadTypes;
use reth_node_api::{BuiltPayload, EngineTypes};
use std::marker::PhantomData;

use reth_arbitrum_payload::ArbExecutionData;

#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize)]
#[non_exhaustive]
pub struct ArbEngineTypes<T: PayloadTypes = reth_node_api::payload::DefaultPayloadTypes> {
    _marker: PhantomData<T>,
}

impl<T: PayloadTypes<ExecutionData = ArbExecutionData>> EngineTypes for ArbEngineTypes<T>
where
    T::BuiltPayload: BuiltPayload
        + TryInto<ExecutionPayloadV1>
        + TryInto<ExecutionPayloadEnvelopeV2>,
{
    type ExecutionPayloadEnvelopeV1 = ExecutionPayloadV1;
    type ExecutionPayloadEnvelopeV2 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV3 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV4 = ExecutionPayloadEnvelopeV2;
    type ExecutionPayloadEnvelopeV5 = ExecutionPayloadEnvelopeV2;
}
