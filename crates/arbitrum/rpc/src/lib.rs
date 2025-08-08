#![cfg_attr(not(feature = "std"), no_std)]

pub mod engine {
    pub const ARB_ENGINE_CAPABILITIES: &[&str] = &[
        "engine_forkchoiceUpdatedV1",
        "engine_forkchoiceUpdatedV2",
        "engine_forkchoiceUpdatedV3",
        "engine_getClientVersionV1",
        "engine_getPayloadV2",
        "engine_getPayloadV3",
        "engine_getPayloadV4",
        "engine_newPayloadV2",
        "engine_newPayloadV3",
        "engine_newPayloadV4",
        "engine_getPayloadBodiesByHashV1",
        "engine_getPayloadBodiesByRangeV1",
    ];
}

pub struct ArbRpc;
