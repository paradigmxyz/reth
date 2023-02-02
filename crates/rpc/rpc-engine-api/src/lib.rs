#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! The implementation of Engine API.
//! [Read more](https://github.com/ethereum/execution-apis/tree/main/src/engine).

/// The Engine API implementation.
pub mod engine_api;

/// The Engine API message type.
pub mod message;

/// Engine API error.
pub mod error;

pub use engine_api::{EngineApi, EngineApiSender};
pub use error::{EngineApiError, EngineApiResult};
pub use message::EngineApiMessage;
