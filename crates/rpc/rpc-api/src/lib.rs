#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxzy/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth RPC interface definitions
//!
//! Provides all RPC interfaces.
//!
//! ## Feature Flags
//!
//! - `client`: Enables JSON-RPC client support.

mod admin;
mod debug;
mod engine;
mod eth;
mod eth_filter;
mod eth_pubsub;
mod net;
mod rpc;
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
        rpc::RpcApiServer,
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
        rpc::RpcApiServer,
        trace::TraceApiClient,
        txpool::TxPoolApiClient,
        web3::Web3ApiClient,
    };
}
