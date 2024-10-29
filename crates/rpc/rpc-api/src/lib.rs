//! Reth RPC interface definitions
//!
//! Provides all RPC interfaces.
//!
//! ## Feature Flags
//!
//! - `client`: Enables JSON-RPC client support.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod admin;
mod anvil;
mod debug;
mod engine;
mod ganache;
mod hardhat;
mod mev;
mod net;
mod otterscan;
mod reth;
mod rpc;
mod trace;
mod txpool;
mod validation;
mod web3;

/// re-export of all server traits
pub use servers::*;

/// Aggregates all server traits.
pub mod servers {
    pub use crate::{
        admin::AdminApiServer,
        debug::DebugApiServer,
        engine::{EngineApiServer, EngineEthApiServer},
        mev::{MevFullApiServer, MevSimApiServer},
        net::NetApiServer,
        otterscan::OtterscanServer,
        reth::RethApiServer,
        rpc::RpcApiServer,
        trace::TraceApiServer,
        txpool::TxPoolApiServer,
        validation::BlockSubmissionValidationApiServer,
        web3::Web3ApiServer,
    };
    pub use reth_rpc_eth_api::{
        self as eth, EthApiServer, EthBundleApiServer, EthCallBundleApiServer, EthFilterApiServer,
        EthPubSubApiServer,
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
        anvil::AnvilApiClient,
        debug::DebugApiClient,
        engine::{EngineApiClient, EngineEthApiClient},
        ganache::GanacheApiClient,
        hardhat::HardhatApiClient,
        mev::{MevFullApiClient, MevSimApiClient},
        net::NetApiClient,
        otterscan::OtterscanClient,
        reth::RethApiClient,
        rpc::RpcApiServer,
        trace::TraceApiClient,
        txpool::TxPoolApiClient,
        validation::BlockSubmissionValidationApiClient,
        web3::Web3ApiClient,
    };
    pub use reth_rpc_eth_api::{
        EthApiClient, EthBundleApiClient, EthCallBundleApiClient, EthFilterApiClient,
    };
}
