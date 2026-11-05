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

//! jsonrpsee wrapper crate.
//!
//! <br>
//!
//! # Optional features
//!
//! The `jsonrpsee` crate composes JSON-RPC functionality behind optional feature
//! flags to provide for client and server communication over specific protocols.
//! There are no default features, all functionality must be opted in to accordingly.
//! The following features are available.
//!
//! - **`http-client`** - JSON-RPC client functionality over HTTP protocol.
//! - **`wasm-client`** - JSON-RPC client functionality over web-sys.
//! - **`ws-client`** - JSON-RPC client functionality over WebSocket protocol.
//! - **`macros`** - JSON-RPC API generation convenience by derive macros.
//! - **`client-core`** - Enables minimal client features to generate the RPC API without
//!   transports.
//! - **`client`** - Enables all client features including transports.
//! - **`server-core`** - Enables minimal server features to generate the RPC API without
//!   transports.
//! - **`server`** - Enables all server features including transports.
//! - **`full`** - Enables all features.
//! - **`async-client`** - Enables the async client without any transport.
//! - **`client-ws-transport`** - Enables `ws` transport with TLS.
//! - **`client-ws-transport-no-tls`** - Enables `ws` transport without TLS.
//! - **`client-web-transport`** - Enables `websys` transport.
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

// Macros useful below, but not to be exposed outside of the crate.
#[macro_use]
mod macros;

cfg_http_client! {
    pub use jsonrpsee_http_client as http_client;
}

cfg_ws_client! {
    pub use jsonrpsee_ws_client as ws_client;
}

cfg_wasm_client! {
    pub use jsonrpsee_wasm_client as wasm_client;
}

cfg_async_client! {
    pub use jsonrpsee_core::client::async_client;
}

cfg_client_transport! {
    pub use jsonrpsee_client_transport as client_transport;
}

cfg_server! {
    pub use jsonrpsee_server as server;
    pub use tokio;
}

cfg_server_core! {
    pub use jsonrpsee_core::server::*;
}

cfg_proc_macros! {
    pub use jsonrpsee_proc_macros as proc_macros;
    pub use tracing;
}

cfg_types! {
    pub use jsonrpsee_types as types;
}

cfg_client_or_server! {
    pub use jsonrpsee_core as core;
}

cfg_client! {
    pub use jsonrpsee_core::rpc_params;
}
