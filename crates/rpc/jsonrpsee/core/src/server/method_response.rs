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

//! Represent a method response.

const LOG_TARGET: &str = "jsonrpsee-core";

use std::io;
use std::task::Poll;

use crate::traits::ToJson;

use futures_util::{Future, FutureExt};
use http::Extensions;
use jsonrpsee_types::error::{
	ErrorCode, ErrorObject, OVERSIZED_RESPONSE_CODE, OVERSIZED_RESPONSE_MSG, reject_too_big_batch_response,
};
use jsonrpsee_types::{ErrorObjectOwned, Id, Response, ResponsePayload as InnerResponsePayload};
use serde::Serialize;
use serde_json::value::{RawValue, to_raw_value};

#[derive(Debug, Clone)]
enum ResponseKind {
	MethodCall,
	Subscription,
	Batch,
	Notification,
}

/// Represents a response to a method call.
///
/// NOTE: A subscription is also a method call but it's
/// possible determine whether a method response
/// is "subscription" or "ordinary method call"
/// by calling [`MethodResponse::is_subscription`]
#[derive(Debug)]
pub struct MethodResponse {
	/// Serialized JSON-RPC response,
	json: Box<RawValue>,
	/// Indicates whether the call was successful or not.
	success_or_error: MethodResponseResult,
	/// Indicates whether the call was a subscription response.
	kind: ResponseKind,
	/// Optional callback that may be utilized to notif
	/// that the method response has been processed
	on_close: Option<MethodResponseNotifyTx>,
	/// The response's extensions.
	extensions: Extensions,
}

impl AsRef<str> for MethodResponse {
	fn as_ref(&self) -> &str {
		self.json.get()
	}
}

impl ToJson for MethodResponse {
	fn to_json(&self) -> Result<Box<RawValue>, serde_json::Error> {
		Ok(self.json.clone())
	}
}

impl std::fmt::Display for MethodResponse {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "{}", self.json)
	}
}

impl MethodResponse {
	/// Returns whether the call was successful.
	pub fn is_success(&self) -> bool {
		self.success_or_error.is_success()
	}

	/// Returns whether the call failed.
	pub fn is_error(&self) -> bool {
		self.success_or_error.is_error()
	}

	/// Returns whether the response is a subscription response.
	pub fn is_subscription(&self) -> bool {
		matches!(self.kind, ResponseKind::Subscription)
	}

	/// Returns whether the response is a method response.
	pub fn is_method_call(&self) -> bool {
		matches!(self.kind, ResponseKind::MethodCall)
	}

	/// Returns whether the response is a notification response.
	pub fn is_notification(&self) -> bool {
		matches!(self.kind, ResponseKind::Notification)
	}

	/// Returns whether the response is a batch response.
	pub fn is_batch(&self) -> bool {
		matches!(self.kind, ResponseKind::Batch)
	}

	/// Consume the method response and extract the serialized JSON response.
	pub fn into_json(self) -> Box<RawValue> {
		self.json
	}

	/// Get the serialized JSON response.
	pub fn to_json(&self) -> Box<RawValue> {
		self.json.clone()
	}

	/// Consume the method response and extract the parts.
	pub fn into_parts(self) -> (Box<RawValue>, Option<MethodResponseNotifyTx>, Extensions) {
		(self.json, self.on_close, self.extensions)
	}

	/// Get the error code
	///
	/// Returns `Some(error code)` if the call failed.
	pub fn as_error_code(&self) -> Option<i32> {
		self.success_or_error.as_error_code()
	}

	/// Get a reference to the serialized JSON response.
	pub fn as_json(&self) -> &RawValue {
		&self.json
	}

	/// Create a method response from [`BatchResponse`].
	pub fn from_batch(batch: BatchResponse) -> Self {
		Self {
			json: batch.json,
			success_or_error: MethodResponseResult::Success,
			kind: ResponseKind::Batch,
			on_close: None,
			extensions: batch.extensions,
		}
	}

	/// This is similar to [`MethodResponse::response`] but sets a flag to indicate
	/// that response is a subscription.
	pub fn subscription_response<T>(id: Id, result: ResponsePayload<T>, max_response_size: usize) -> Self
	where
		T: Serialize + Clone,
	{
		let mut rp = Self::response(id, result, max_response_size);
		rp.kind = ResponseKind::Subscription;
		rp
	}

	/// Create a new method response.
	///
	/// If the serialization of `result` exceeds `max_response_size` then
	/// the response is changed to an JSON-RPC error object.
	pub fn response<T>(id: Id, rp: ResponsePayload<T>, max_response_size: usize) -> Self
	where
		T: Serialize + Clone,
	{
		let mut writer = BoundedWriter::new(max_response_size);

		let success_or_error = if let InnerResponsePayload::Error(ref e) = rp.inner {
			MethodResponseResult::Failed(e.code())
		} else {
			MethodResponseResult::Success
		};

		let kind = ResponseKind::MethodCall;

		match serde_json::to_writer(&mut writer, &Response::new(rp.inner, id.clone())) {
			Ok(_) => {
				// Safety - serde_json does not emit invalid UTF-8.
				let result = unsafe { String::from_utf8_unchecked(writer.into_bytes()) };
				let json = RawValue::from_string(result).expect("Valid JSON String; qed");

				Self { json, success_or_error, kind, on_close: rp.on_exit, extensions: Extensions::new() }
			}
			Err(err) => {
				tracing::error!(target: LOG_TARGET, "Error serializing response: {:?}", err);

				if err.is_io() {
					let data = to_raw_value(&format!("Exceeded max limit of {max_response_size}")).ok();
					let err_code = OVERSIZED_RESPONSE_CODE;

					let err = InnerResponsePayload::<()>::error_borrowed(ErrorObject::borrowed(
						err_code,
						OVERSIZED_RESPONSE_MSG,
						data.as_deref(),
					));
					let json = serde_json::value::to_raw_value(&Response::new(err, id))
						.expect("JSON serialization infallible; qed");

					Self {
						json,
						success_or_error: MethodResponseResult::Failed(err_code),
						kind,
						on_close: rp.on_exit,
						extensions: Extensions::new(),
					}
				} else {
					let err = ErrorCode::InternalError;
					let payload = jsonrpsee_types::ResponsePayload::<()>::error(err);
					let json = serde_json::value::to_raw_value(&Response::new(payload, id))
						.expect("JSON serialization infallible; qed");
					Self {
						json,
						success_or_error: MethodResponseResult::Failed(err.code()),
						kind,
						on_close: rp.on_exit,
						extensions: Extensions::new(),
					}
				}
			}
		}
	}

	/// This is similar to [`MethodResponse::error`] but sets a flag to indicate
	/// that error is a subscription.
	pub fn subscription_error<'a>(id: Id, err: impl Into<ErrorObject<'a>>) -> Self {
		let mut rp = Self::error(id, err);
		rp.kind = ResponseKind::Subscription;
		rp
	}

	/// Create a [`MethodResponse`] from a JSON-RPC error.
	pub fn error<'a>(id: Id, err: impl Into<ErrorObject<'a>>) -> Self {
		let err: ErrorObject = err.into();
		let err_code = err.code();
		let err = InnerResponsePayload::<()>::error_borrowed(err);
		let json =
			serde_json::value::to_raw_value(&Response::new(err, id)).expect("JSON serialization infallible; qed");
		Self {
			json,
			success_or_error: MethodResponseResult::Failed(err_code),
			kind: ResponseKind::MethodCall,
			on_close: None,
			extensions: Extensions::new(),
		}
	}

	/// Create notification response which is a response that doesn't expect a reply.
	pub fn notification() -> Self {
		Self {
			json: RawValue::NULL.to_owned(),
			success_or_error: MethodResponseResult::Success,
			kind: ResponseKind::Notification,
			on_close: None,
			extensions: Extensions::new(),
		}
	}

	/// Returns a reference to the associated extensions.
	pub fn extensions(&self) -> &Extensions {
		&self.extensions
	}

	/// Returns a mut reference to the associated extensions.
	pub fn extensions_mut(&mut self) -> &mut Extensions {
		&mut self.extensions
	}

	/// Consumes the method response and returns a new one with the given extensions.
	pub fn with_extensions(self, extensions: Extensions) -> Self {
		Self { extensions, ..self }
	}
}

/// Represent the outcome of a method call success or failed.
#[derive(Debug, Copy, Clone)]
enum MethodResponseResult {
	/// The method call was successful.
	Success,
	/// The method call failed with error code.
	Failed(i32),
}

impl MethodResponseResult {
	/// Returns whether the call was successful.
	fn is_success(&self) -> bool {
		matches!(self, MethodResponseResult::Success)
	}

	/// Returns whether the call failed.
	fn is_error(&self) -> bool {
		matches!(self, MethodResponseResult::Failed(_))
	}

	/// Get the error code
	///
	/// Returns `Some(error code)` if the call failed.
	fn as_error_code(&self) -> Option<i32> {
		match self {
			Self::Failed(e) => Some(*e),
			_ => None,
		}
	}
}

/// Builder to build a `BatchResponse`.
#[derive(Debug, Clone, Default)]
pub struct BatchResponseBuilder {
	/// Serialized JSON-RPC response,
	result: String,
	/// Max limit for the batch
	max_response_size: usize,
	/// Extensions for the batch response.
	extensions: Extensions,
}

impl BatchResponseBuilder {
	/// Create a new batch response builder with limit.
	pub fn new_with_limit(limit: usize) -> Self {
		let mut initial = String::with_capacity(2048);
		initial.push('[');

		Self { result: initial, max_response_size: limit, extensions: Extensions::new() }
	}

	/// Append a result from an individual method to the batch response.
	///
	/// Fails if the max limit is exceeded and returns to error response to
	/// return early in order to not process method call responses which are thrown away anyway.
	pub fn append(&mut self, response: MethodResponse) -> Result<(), MethodResponse> {
		// `,` will occupy one extra byte for each entry
		// on the last item the `,` is replaced by `]`.
		let len = response.json.get().len() + self.result.len() + 1;
		self.extensions.extend(response.extensions);

		if len > self.max_response_size {
			Err(MethodResponse::error(Id::Null, reject_too_big_batch_response(self.max_response_size)))
		} else {
			self.result.push_str(response.json.get());
			self.result.push(',');
			Ok(())
		}
	}

	/// Check if the batch is empty.
	pub fn is_empty(&self) -> bool {
		self.result.len() <= 1
	}

	/// Finish the batch response
	pub fn finish(mut self) -> BatchResponse {
		if self.result.len() == 1 {
			BatchResponse {
				json: batch_response_error(Id::Null, ErrorObject::from(ErrorCode::InvalidRequest)),
				extensions: self.extensions,
			}
		} else {
			self.result.pop();
			self.result.push(']');
			let json = RawValue::from_string(self.result).expect("BatchResponse builds a valid JSON String; qed");
			BatchResponse { json, extensions: self.extensions }
		}
	}
}

/// Serialized batch response.
#[derive(Debug, Clone)]
pub struct BatchResponse {
	json: Box<RawValue>,
	extensions: Extensions,
}

/// Create a JSON-RPC error response.
pub fn batch_response_error(id: Id, err: impl Into<ErrorObject<'static>>) -> Box<RawValue> {
	let err = InnerResponsePayload::<()>::error_borrowed(err);
	serde_json::value::to_raw_value(&Response::new(err, id)).expect("JSON serialization infallible; qed")
}

/// Similar to [`jsonrpsee_types::ResponsePayload`] but possible to with an async-like
/// API to detect when a method response has been sent.
#[derive(Debug)]
pub struct ResponsePayload<'a, T>
where
	T: Clone,
{
	inner: InnerResponsePayload<'a, T>,
	on_exit: Option<MethodResponseNotifyTx>,
}

impl<'a, T: Clone> From<InnerResponsePayload<'a, T>> for ResponsePayload<'a, T> {
	fn from(inner: InnerResponsePayload<'a, T>) -> Self {
		Self { inner, on_exit: None }
	}
}

impl<'a, T> ResponsePayload<'a, T>
where
	T: Clone,
{
	/// Create a successful owned response payload.
	pub fn success(t: T) -> Self {
		InnerResponsePayload::success(t).into()
	}

	/// Create a successful borrowed response payload.
	pub fn success_borrowed(t: &'a T) -> Self {
		InnerResponsePayload::success_borrowed(t).into()
	}

	/// Create an error response payload.
	pub fn error(e: impl Into<ErrorObjectOwned>) -> Self {
		InnerResponsePayload::error(e.into()).into()
	}

	/// Create a borrowd error response payload.
	pub fn error_borrowed(e: impl Into<ErrorObject<'a>>) -> Self {
		InnerResponsePayload::error_borrowed(e.into()).into()
	}

	/// Consumes the [`ResponsePayload`] and produces new [`ResponsePayload`] and a future
	/// [`MethodResponseFuture`] that will be resolved once the response has been processed.
	///
	/// If this has been called more than once then this will overwrite
	/// the old result the previous future(s) will be resolved with error.
	pub fn notify_on_completion(mut self) -> (Self, MethodResponseFuture) {
		let (tx, rx) = response_channel();
		self.on_exit = Some(tx);
		(self, rx)
	}

	/// Convert the response payload into owned.
	pub fn into_owned(self) -> ResponsePayload<'static, T> {
		ResponsePayload { inner: self.inner.into_owned(), on_exit: self.on_exit }
	}
}

impl<T> From<ErrorCode> for ResponsePayload<'_, T>
where
	T: Clone,
{
	fn from(code: ErrorCode) -> Self {
		let err: ErrorObject = code.into();
		Self::error(err)
	}
}

/// Create a channel to be used in combination with [`ResponsePayload`] to
/// notify when a method call has been processed.
fn response_channel() -> (MethodResponseNotifyTx, MethodResponseFuture) {
	let (tx, rx) = tokio::sync::oneshot::channel();
	(MethodResponseNotifyTx(tx), MethodResponseFuture(rx))
}

/// Sends a message once the method response has been processed.
#[derive(Debug)]
pub struct MethodResponseNotifyTx(tokio::sync::oneshot::Sender<NotifyMsg>);

impl MethodResponseNotifyTx {
	/// Send a notify message.
	pub fn notify(self, is_success: bool) {
		let msg = if is_success { NotifyMsg::Ok } else { NotifyMsg::Err };
		_ = self.0.send(msg);
	}
}

/// Future that resolves when the method response has been processed.
#[derive(Debug)]
pub struct MethodResponseFuture(tokio::sync::oneshot::Receiver<NotifyMsg>);

/// A message that that tells whether notification
/// was succesful or not.
#[derive(Debug, Copy, Clone)]
pub enum NotifyMsg {
	/// The response was successfully processed.
	Ok,
	/// The response was the wrong kind
	/// such an error response when
	/// one expected a successful response.
	Err,
}

/// Method response error.
#[derive(Debug, Copy, Clone)]
pub enum MethodResponseError {
	/// The connection was closed.
	Closed,
	/// The response was a JSON-RPC error.
	JsonRpcError,
}

impl Future for MethodResponseFuture {
	type Output = Result<(), MethodResponseError>;

	fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
		match self.0.poll_unpin(cx) {
			Poll::Ready(Ok(NotifyMsg::Ok)) => Poll::Ready(Ok(())),
			Poll::Ready(Ok(NotifyMsg::Err)) => Poll::Ready(Err(MethodResponseError::JsonRpcError)),
			Poll::Ready(Err(_)) => Poll::Ready(Err(MethodResponseError::Closed)),
			Poll::Pending => Poll::Pending,
		}
	}
}

/// Bounded writer that allows writing at most `max_len` bytes.
#[derive(Debug, Clone)]
struct BoundedWriter {
	max_len: usize,
	buf: Vec<u8>,
}

impl BoundedWriter {
	/// Create a new bounded writer.
	pub fn new(max_len: usize) -> Self {
		Self { max_len, buf: Vec::with_capacity(128) }
	}

	/// Consume the writer and extract the written bytes.
	pub fn into_bytes(self) -> Vec<u8> {
		self.buf
	}
}

impl io::Write for &mut BoundedWriter {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		let len = self.buf.len() + buf.len();
		if self.max_len >= len {
			self.buf.extend_from_slice(buf);
			Ok(buf.len())
		} else {
			Err(io::Error::new(io::ErrorKind::OutOfMemory, "Memory capacity exceeded"))
		}
	}

	fn flush(&mut self) -> io::Result<()> {
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::{BatchResponseBuilder, BoundedWriter, Id, MethodResponse, ResponsePayload};

	#[test]
	fn batch_with_single_works() {
		let method = MethodResponse::response(Id::Number(1), ResponsePayload::success_borrowed(&"a"), usize::MAX);
		assert_eq!(method.json.get().len(), 37);

		// Recall a batch appends two bytes for the `[]`.
		let mut builder = BatchResponseBuilder::new_with_limit(39);
		builder.append(method).unwrap();
		let batch = builder.finish();

		assert_eq!(batch.json.get(), r#"[{"jsonrpc":"2.0","id":1,"result":"a"}]"#)
	}

	#[test]
	fn batch_with_multiple_works() {
		let m1 = MethodResponse::response(Id::Number(1), ResponsePayload::success_borrowed(&"a"), usize::MAX);
		let m11 = MethodResponse::response(Id::Number(1), ResponsePayload::success_borrowed(&"a"), usize::MAX);
		assert_eq!(m1.json.get().len(), 37);

		// Recall a batch appends two bytes for the `[]` and one byte for `,` to append a method call.
		// so it should be 2 + (37 * n) + (n-1)
		let limit = 2 + (37 * 2) + 1;
		let mut builder = BatchResponseBuilder::new_with_limit(limit);
		builder.append(m1).unwrap();
		builder.append(m11).unwrap();
		let batch = builder.finish();

		assert_eq!(batch.json.get(), r#"[{"jsonrpc":"2.0","id":1,"result":"a"},{"jsonrpc":"2.0","id":1,"result":"a"}]"#)
	}

	#[test]
	fn batch_empty_err() {
		let batch = BatchResponseBuilder::new_with_limit(1024).finish();

		let exp_err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}"#;
		assert_eq!(batch.json.get(), exp_err);
	}

	#[test]
	fn batch_too_big() {
		let method = MethodResponse::response(Id::Number(1), ResponsePayload::success_borrowed(&"a".repeat(28)), 128);
		assert_eq!(method.json.get().len(), 64);

		let batch = BatchResponseBuilder::new_with_limit(63).append(method).unwrap_err();

		let exp_err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32011,"message":"The batch response was too large","data":"Exceeded max limit of 63"}}"#;
		assert_eq!(batch.json.get(), exp_err);
	}

	#[test]
	fn bounded_serializer_work() {
		use jsonrpsee_types::{Response, ResponsePayload};

		let mut writer = BoundedWriter::new(100);
		let result = ResponsePayload::success(&"success");
		let rp = &Response::new(result, Id::Number(1));

		assert!(serde_json::to_writer(&mut writer, rp).is_ok());
		assert_eq!(String::from_utf8(writer.into_bytes()).unwrap(), r#"{"jsonrpc":"2.0","id":1,"result":"success"}"#);
	}

	#[test]
	fn bounded_serializer_cap_works() {
		let mut writer = BoundedWriter::new(100);
		// NOTE: `"` is part of the serialization so 101 characters.
		assert!(serde_json::to_writer(&mut writer, &"x".repeat(99)).is_err());
	}
}
