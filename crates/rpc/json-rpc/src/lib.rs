//! Minimal JSON-RPC 2.0 server and client.
//!
//! Requests and their parameters are slices of the received buffer, so no request data is copied
//! before a method deserializes it.
//!
//! ## Feature Flags
//!
//! - `macros`: The [`rpc`] attribute macro.
//! - `server`: HTTP and `WebSocket` server.
//! - `client`: Client traits and the generic async client.
//! - `http-client`: HTTP client.
//! - `ws-client`: `WebSocket` client.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod config;
pub use config::ServerConfig;

mod connection;
pub use connection::serve_connection;

mod error;
pub use error::*;

mod handle;
pub use handle::{stop_channel, AlreadyStoppedError, ServerHandle, StopHandle};

mod id;
pub use id::{Id, SubscriptionId};

mod middleware;
pub use middleware::{RpcService, RpcServiceBuilder, RpcServiceT};

mod module;
pub use module::{IntoResponse, RegisterMethodError, RpcModule};

mod params;
pub use params::{Params, ParamsSequence};

mod request;
pub use request::{ByteStr, Request};

mod response;
pub use response::MethodResponse;

mod subscription;
pub use subscription::{
    DisconnectError, IdProvider, IntoSubscriptionResult, PendingSubscriptionSink,
    RandomIntegerIdProvider, RandomStringIdProvider, StringError, SubscriptionResult,
    SubscriptionSink,
};

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "macros")]
pub use reth_json_rpc_macros::rpc;

pub use serde::{de::DeserializeOwned, Serialize};
pub use serde_json::value::RawValue;

/// Result of an RPC method.
pub type RpcResult<T> = Result<T, ErrorObject>;

#[doc(hidden)]
pub mod __private {
    pub use crate::module::MethodResult;
}
