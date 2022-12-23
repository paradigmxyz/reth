#![warn(missing_debug_implementations, missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth RPC interface definitions
//!
//! Provides all RPC interfaces.

mod admin;
mod debug;
mod engine;
mod eth;
mod eth_filter;
mod eth_pubsub;
mod net;
mod trace;
mod web3;

pub use self::{
    debug::DebugApiServer, engine::EngineApiServer, eth::EthApiServer,
    eth_filter::EthFilterApiServer, eth_pubsub::EthPubSubApiServer, net::NetApiServer,
    web3::Web3ApiServer,
};
