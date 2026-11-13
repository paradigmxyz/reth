// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! # jsonrpsee-server
//!
//! `jsonrpsee-server` is a [JSON RPC](https://www.jsonrpc.org/specification) server that supports both HTTP and WebSocket transport.
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod future;
mod server;
mod transport;
mod utils;

pub mod middleware;

#[cfg(test)]
mod tests;

pub use future::{
    stop_channel, AlreadyStoppedError, ConnectionGuard, ConnectionPermit, ServerHandle, StopHandle,
};
pub use jsonrpsee_core::{
    error::RegisterMethodError, id_providers::*, server::*, traits::IdProvider,
};
pub use jsonrpsee_types as types;
pub use server::{
    handle_rpc_call, BatchRequestConfig, Builder as ServerBuilder, ConnectionState, Server,
    ServerConfig, ServerConfigBuilder, TowerServiceNoHttp,
};
pub use tracing;

pub use jsonrpsee_core::http_helpers::{
    Body as HttpBody, Request as HttpRequest, Response as HttpResponse,
};
pub use transport::{http, stream, ws};
pub use utils::{accept_error_backoff, serve_with_graceful_shutdown};

pub(crate) const LOG_TARGET: &str = "jsonrpsee-server";
