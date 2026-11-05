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

//! Error type for client(s).

use crate::{BoxError, RegisterMethodError, params::EmptyBatchRequest};
use jsonrpsee_types::{ErrorObjectOwned, InvalidRequestId};
use std::sync::Arc;

/// Error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	/// JSON-RPC error which can occur when a JSON-RPC call fails.
	#[error("{0}")]
	Call(#[from] ErrorObjectOwned),
	/// Networking error or error on the low-level protocol layer.
	#[error(transparent)]
	Transport(BoxError),
	/// The background task has been terminated.
	#[error("The background task closed {0}; restart required")]
	RestartNeeded(Arc<Error>),
	/// Failed to parse the data.
	#[error("Parse error: {0}")]
	ParseError(#[from] serde_json::Error),
	/// Invalid subscription ID.
	#[error("Invalid subscription ID")]
	InvalidSubscriptionId,
	/// Invalid request ID.
	#[error(transparent)]
	InvalidRequestId(#[from] InvalidRequestId),
	/// Request timeout
	#[error("Request timeout")]
	RequestTimeout,
	/// Custom error.
	#[error("Custom error: {0}")]
	Custom(String),
	/// Not implemented for HTTP clients.
	#[error("Not implemented")]
	HttpNotImplemented,
	/// Empty batch request.
	#[error(transparent)]
	EmptyBatchRequest(#[from] EmptyBatchRequest),
	/// The error returned when registering a method or subscription failed.
	#[error(transparent)]
	RegisterMethod(#[from] RegisterMethodError),
	/// An internal state when the underlying RpcService
	/// got disconnected and the error must be fetched
	/// from the backend.
	//
	// NOTE: This is a workaround where an error occurred in
	// underlying RpcService implementation in the async client
	// but we don't want to expose different error types for
	// ergonomics when writing middleware.
	#[error("RPC service disconnected")]
	#[doc(hidden)]
	ServiceDisconnect,
}
