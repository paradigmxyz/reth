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
//! flags to provide for server communication over specific protocols.
//! There are no default features, all functionality must be opted in to accordingly.
//! The following features are available.
//!
//! - **`macros`** - JSON-RPC API generation convenience by derive macros.
//! - **`server-core`** - Enables minimal server features to generate the RPC API without
//!   transports.
//! - **`server`** - Enables all server features including transports.
//! - **`full`** - Enables all features.
//!
//! Vendored from [jsonrpsee](https://github.com/paritytech/jsonrpsee), Copyright (c) 2019-2021
//! Parity Technologies (UK) Ltd., licensed under the MIT License (see
//! `crates/rpc/jsonrpsee/LICENSE`).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "server")]
pub use jsonrpsee_server as server;
#[cfg(feature = "server")]
pub use tokio;

#[cfg(feature = "server-core")]
pub use jsonrpsee_core::server::*;

#[cfg(feature = "jsonrpsee-proc-macros")]
pub use jsonrpsee_proc_macros as proc_macros;
#[cfg(feature = "jsonrpsee-proc-macros")]
pub use tracing;

#[cfg(feature = "jsonrpsee-types")]
pub use jsonrpsee_types as types;

#[cfg(feature = "server-core")]
pub use jsonrpsee_core as core;
