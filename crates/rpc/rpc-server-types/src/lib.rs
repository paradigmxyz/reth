//! Reth RPC server types.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

/// Common RPC constants.
pub mod constants;

mod config;
pub use config::RpcServerConfig;

/// Cors utilities.
mod cors;

/// Rpc error utilities.
pub mod error;

// Rpc server metrics
mod metrics;

mod module;
pub use module::RethRpcModule;

mod rpc_server;
pub use rpc_server::RpcServer;

mod ws_http_server;
pub use ws_http_server::WsHttpServer;

pub use jsonrpsee::server::ServerBuilder;
