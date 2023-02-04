#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
// TODO remove later
#![allow(dead_code)]

//! Reth RPC implementation
//!
//! Provides the implementation of all RPC interfaces.

mod admin;
mod debug;
mod engine;
mod eth;
mod layers;
mod net;
mod trace;
mod web3;

pub use admin::AdminApi;
pub use debug::DebugApi;
pub use engine::EngineApi;
pub use eth::{EthApi, EthApiSpec, EthPubSub};
pub use layers::{AuthLayer, AuthValidator, JwtAuthValidator, JwtError, JwtSecret};
pub use net::NetApi;
pub use trace::TraceApi;
pub use web3::Web3Api;

pub(crate) mod result;
