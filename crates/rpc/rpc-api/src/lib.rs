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
mod txpool;
mod web3;

/// re-export of all server traits
pub use servers::*;

/// Aggregates all server traits.
pub mod servers {
    pub use crate::{
        admin::AdminApiServer,
        debug::DebugApiServer,
        engine::{EngineApiServer, EngineEthApiServer},
        eth::EthApiServer,
        eth_filter::EthFilterApiServer,
        eth_pubsub::EthPubSubApiServer,
        net::NetApiServer,
        trace::TraceApiServer,
        txpool::TxPoolApiServer,
        web3::Web3ApiServer,
    };
}

/// re-export of all client traits
#[cfg(feature = "client")]
pub use clients::*;

/// Aggregates all client traits.
#[cfg(feature = "client")]
pub mod clients {
    pub use crate::{
        admin::AdminApiClient,
        debug::DebugApiClient,
        engine::{EngineApiClient, EngineEthApiClient},
        eth::EthApiClient,
        net::NetApiClient,
        trace::TraceApiClient,
        txpool::TxPoolApiClient,
        web3::Web3ApiClient,
    };
}
