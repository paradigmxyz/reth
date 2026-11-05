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

use std::time::Duration;

use jsonrpsee_types::{ErrorCode, ErrorObject, Id, InvalidRequest, Response, ResponsePayload};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use super::{DisconnectError, SendTimeoutError, TrySendError};

/// Sink that is used to send back the result to the server for a specific method.
#[derive(Clone, Debug)]
pub struct MethodSink {
	/// Channel sender.
	tx: mpsc::Sender<Box<RawValue>>,
	/// Max response size in bytes for a executed call.
	max_response_size: u32,
}

impl MethodSink {
	/// Create a new `MethodSink` with unlimited response size.
	pub fn new(tx: mpsc::Sender<Box<RawValue>>) -> Self {
		MethodSink { tx, max_response_size: u32::MAX }
	}

	/// Create a new `MethodSink` with a limited response size.
	pub fn new_with_limit(tx: mpsc::Sender<Box<RawValue>>, max_response_size: u32) -> Self {
		MethodSink { tx, max_response_size }
	}

	/// Returns whether this channel is closed without needing a context.
	pub fn is_closed(&self) -> bool {
		self.tx.is_closed()
	}

	/// Same as [`tokio::sync::mpsc::Sender::closed`].
	///
	/// # Cancel safety
	/// This method is cancel safe. Once the channel is closed,
	/// it stays closed forever and all future calls to closed will return immediately.
	pub async fn closed(&self) {
		self.tx.closed().await
	}

	/// Get the max response size.
	pub const fn max_response_size(&self) -> u32 {
		self.max_response_size
	}

	/// Attempts to send out the message immediately and fails if the underlying
	/// connection has been closed or if the message buffer is full.
	///
	/// Returns the message if the send fails such that either can be thrown away or re-sent later.
	pub fn try_send(&mut self, msg: Box<RawValue>) -> Result<(), TrySendError> {
		self.tx.try_send(msg).map_err(Into::into)
	}

	/// Async send which will wait until there is space in channel buffer or that the subscription is disconnected.
	pub async fn send(&self, msg: Box<RawValue>) -> Result<(), DisconnectError> {
		self.tx.send(msg).await.map_err(Into::into)
	}

	/// Send a JSON-RPC error to the client
	pub async fn send_error<'a>(&self, id: Id<'a>, err: ErrorObject<'a>) -> Result<(), DisconnectError> {
		let payload = ResponsePayload::<()>::error_borrowed(err);
		let json = serde_json::value::to_raw_value(&Response::new(payload, id)).expect("valid JSON; qed");

		self.send(json).await
	}

	/// Similar to `MethodSink::send` but only waits for a limited time.
	pub async fn send_timeout(&self, msg: Box<RawValue>, timeout: Duration) -> Result<(), SendTimeoutError> {
		self.tx.send_timeout(msg, timeout).await.map_err(Into::into)
	}

	/// Get the capacity of the channel.
	pub fn capacity(&self) -> usize {
		self.tx.capacity()
	}

	/// Get the max capacity of the channel.
	pub fn max_capacity(&self) -> usize {
		self.tx.max_capacity()
	}

	/// Waits for there to be space on the return channel.
	pub async fn has_capacity(&self) -> Result<(), DisconnectError> {
		match self.tx.reserve().await {
			// The permit is thrown away here because it's just
			// a way to ensure that the return buffer has space.
			Ok(_) => Ok(()),
			Err(_) => Err(DisconnectError(RawValue::NULL.to_owned().into())),
		}
	}
}

/// Distinguish malformed JSON from an invalid request and recover its [`Id`] when possible.
pub fn prepare_error(data: &[u8]) -> (Id<'_>, ErrorCode) {
	let Ok(value) = serde_json::from_slice::<&RawValue>(data) else {
		return (Id::Null, ErrorCode::ParseError);
	};
	let id = if value.get().starts_with('{') {
		serde_json::from_slice::<InvalidRequest>(data).map(|request| request.id).unwrap_or(Id::Null)
	} else {
		Id::Null
	};
	(id, ErrorCode::InvalidRequest)
}

#[cfg(test)]
mod tests {
	use super::prepare_error;
	use jsonrpsee_types::{ErrorCode, Id};

	#[test]
	fn error_classification_invalid_envelopes_without_id() {
		for data in [r#"{"jsonrpc":"1.0","method":"example","params":[]}"#, r#"{"jsonrpc":"2.0"}"#, r#"{}"#] {
			assert_eq!(prepare_error(data.as_bytes()), (Id::Null, ErrorCode::InvalidRequest), "{data}");
		}
	}

	#[test]
	fn error_classification_malformed_json() {
		for data in ["", " ", "{", "[", r#"{"id":1,"method":}"#, r#"{"id":1} trailing"#] {
			assert_eq!(prepare_error(data.as_bytes()), (Id::Null, ErrorCode::ParseError), "{data}");
		}
	}

	#[test]
	fn error_classification_recoverable_ids() {
		for (data, id) in [
			(r#"{"jsonrpc":"1.0","method":"example","id":0}"#, Id::Number(0)),
			(r#"{"jsonrpc":"1.0","id":18446744073709551615}"#, Id::Number(u64::MAX)),
			(" \n {\"id\":\"keep-id\"} \t ", Id::Str("keep-id".into())),
			(r#"{"id":"escaped\u002did"}"#, Id::Str("escaped-id".into())),
			(r#"{"id":null}"#, Id::Null),
		] {
			assert_eq!(prepare_error(data.as_bytes()), (id, ErrorCode::InvalidRequest), "{data}");
		}
	}

	#[test]
	fn error_classification_unrecoverable_ids_and_nonobjects() {
		for data in [
			r#"{"id":true}"#,
			r#"{"id":{}}"#,
			r#"{"id":[]}"#,
			r#"{"id":-1}"#,
			r#"["not-an-object-id"]"#,
			"[1]",
			"[]",
			"true",
			"null",
			"42",
			r#""text""#,
		] {
			assert_eq!(prepare_error(data.as_bytes()), (Id::Null, ErrorCode::InvalidRequest), "{data}");
		}
	}
}
