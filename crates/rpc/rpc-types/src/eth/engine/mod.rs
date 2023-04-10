//! Engine API types: <https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md> and <https://eips.ethereum.org/EIPS/eip-3675> following the execution specs <https://github.com/ethereum/execution-apis/tree/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine>

#![allow(missing_docs)]

mod error;
mod forkchoice;
mod payload;
mod transition;

pub use self::{error::*, forkchoice::*, payload::*, transition::*};

/// The list of supported Engine capabilities
pub const CAPABILITIES: [&str; 9] = [
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_exchangeTransitionConfigurationV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
];
