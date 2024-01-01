//! Engine API types:
//! <https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md>
//! and <https://eips.ethereum.org/EIPS/eip-3675>,
//! following the execution specs <https://github.com/ethereum/execution-apis/tree/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine>.

#![allow(missing_docs)]

mod cancun;
mod forkchoice;
pub mod payload;
mod transition;
pub use self::{cancun::*, forkchoice::*, payload::*, transition::*};

/// The list of all supported Engine capabilities available over the engine endpoint.
pub const CAPABILITIES: [&str; 12] = [
    "engine_forkchoiceUpdatedV1",
    "engine_forkchoiceUpdatedV2",
    "engine_forkchoiceUpdatedV3",
    "engine_exchangeTransitionConfigurationV1",
    "engine_getPayloadV1",
    "engine_getPayloadV2",
    "engine_getPayloadV3",
    "engine_newPayloadV1",
    "engine_newPayloadV2",
    "engine_newPayloadV3",
    "engine_getPayloadBodiesByHashV1",
    "engine_getPayloadBodiesByRangeV1",
];
