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

use std::{
    io,
    sync::atomic::{AtomicUsize, Ordering},
};

use jsonrpsee_types::{
    error::{
        reject_too_big_batch_response, ErrorCode, ErrorObject, OVERSIZED_RESPONSE_CODE,
        OVERSIZED_RESPONSE_MSG,
    },
    Id, Response,
};
use serde::Serialize;
use serde_json::value::to_raw_value;

pub use jsonrpsee_types::ResponsePayload;

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
    /// Serialized JSON-RPC response.
    json: String,
    /// Indicates whether the call was successful or not.
    success_or_error: MethodResponseResult,
    /// Indicates whether the call was a subscription response.
    kind: ResponseKind,
}

impl AsRef<str> for MethodResponse {
    fn as_ref(&self) -> &str {
        &self.json
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

    /// Returns whether the response is a batch response.
    pub fn is_batch(&self) -> bool {
        matches!(self.kind, ResponseKind::Batch)
    }

    /// Consume the method response and extract the serialized JSON response.
    pub fn into_json(self) -> String {
        self.json
    }

    /// Get the serialized JSON response.
    pub fn to_json(&self) -> String {
        self.json.clone()
    }

    /// Get the error code
    ///
    /// Returns `Some(error code)` if the call failed.
    pub fn as_error_code(&self) -> Option<i32> {
        self.success_or_error.as_error_code()
    }

    /// Create a method response from [`BatchResponse`].
    pub fn from_batch(batch: BatchResponse) -> Self {
        Self {
            json: batch.json,
            success_or_error: MethodResponseResult::Success,
            kind: ResponseKind::Batch,
        }
    }

    /// This is similar to [`MethodResponse::response`] but sets a flag to indicate
    /// that response is a subscription.
    pub fn subscription_response<T>(
        id: Id,
        result: ResponsePayload<T>,
        max_response_size: usize,
    ) -> Self
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
        Self::response_with_capacity(id, rp, max_response_size, 128)
    }

    /// Same as [`MethodResponse::response`], but sizes the serialization buffer from the method's
    /// previous responses and records the size of this one.
    pub(crate) fn sized_response<T>(
        id: Id,
        rp: ResponsePayload<T>,
        max_response_size: usize,
        size_hint: &ResponseSizeHint,
    ) -> Self
    where
        T: Serialize + Clone,
    {
        let mut rp = Self::response_with_capacity(id, rp, max_response_size, size_hint.get());
        // Responses are never shrunk later, so a response much smaller than the previous one
        // would otherwise keep the unused capacity until it is sent.
        if rp.json.capacity() / 2 > rp.json.len() {
            rp.json.shrink_to_fit();
        }
        size_hint.record(rp.json.len());
        rp
    }

    fn response_with_capacity<T>(
        id: Id,
        rp: ResponsePayload<T>,
        max_response_size: usize,
        capacity: usize,
    ) -> Self
    where
        T: Serialize + Clone,
    {
        let mut writer = BoundedWriter::new(max_response_size, capacity);

        let success_or_error = if let ResponsePayload::Error(e) = &rp {
            MethodResponseResult::Failed(e.code())
        } else {
            MethodResponseResult::Success
        };

        let kind = ResponseKind::MethodCall;

        match serde_json::to_writer(&mut writer, &Response::new(rp, id.clone())) {
            Ok(_) => {
                // SAFETY: serde_json does not emit invalid UTF-8.
                let json = unsafe { String::from_utf8_unchecked(writer.into_bytes()) };

                Self { json, success_or_error, kind }
            }
            Err(err) => {
                tracing::error!(target: LOG_TARGET, "Error serializing response: {:?}", err);

                if err.is_io() {
                    let data =
                        to_raw_value(&format!("Exceeded max limit of {max_response_size}")).ok();
                    let err_code = OVERSIZED_RESPONSE_CODE;

                    let err = ResponsePayload::<()>::error_borrowed(ErrorObject::borrowed(
                        err_code,
                        OVERSIZED_RESPONSE_MSG,
                        data.as_deref(),
                    ));
                    let json = serde_json::to_string(&Response::new(err, id))
                        .expect("JSON serialization infallible; qed");

                    Self { json, success_or_error: MethodResponseResult::Failed(err_code), kind }
                } else {
                    let err = ErrorCode::InternalError;
                    let payload = ResponsePayload::<()>::error(err);
                    let json = serde_json::to_string(&Response::new(payload, id))
                        .expect("JSON serialization infallible; qed");
                    Self { json, success_or_error: MethodResponseResult::Failed(err.code()), kind }
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
        let err = ResponsePayload::<()>::error_borrowed(err);
        let json = serde_json::to_string(&Response::new(err, id))
            .expect("JSON serialization infallible; qed");
        Self {
            json,
            success_or_error: MethodResponseResult::Failed(err_code),
            kind: ResponseKind::MethodCall,
        }
    }

    /// Create notification response which is a response that doesn't expect a reply.
    pub fn notification() -> Self {
        Self {
            json: String::from("null"),
            success_or_error: MethodResponseResult::Success,
            kind: ResponseKind::Notification,
        }
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
}

impl BatchResponseBuilder {
    /// Create a new batch response builder with limit.
    pub fn new_with_limit(limit: usize) -> Self {
        let mut initial = String::with_capacity(2048);
        initial.push('[');

        Self { result: initial, max_response_size: limit }
    }

    /// Append a result from an individual method to the batch response.
    ///
    /// Fails if the max limit is exceeded and returns to error response to
    /// return early in order to not process method call responses which are thrown away anyway.
    pub fn append(&mut self, response: MethodResponse) -> Result<(), MethodResponse> {
        // `,` will occupy one extra byte for each entry
        // on the last item the `,` is replaced by `]`.
        let len = response.json.len() + self.result.len() + 1;

        if len > self.max_response_size {
            Err(MethodResponse::error(
                Id::Null,
                reject_too_big_batch_response(self.max_response_size),
            ))
        } else {
            self.result.push_str(&response.json);
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
        // `Self::default` leaves `result` empty, so anything that `is_empty` regards as
        // empty must be rejected here to keep the invariant relied upon below.
        if self.is_empty() {
            BatchResponse {
                json: batch_response_error(Id::Null, ErrorObject::from(ErrorCode::InvalidRequest)),
            }
        } else {
            self.result.pop();
            self.result.push(']');
            BatchResponse { json: self.result }
        }
    }
}

/// Serialized batch response.
#[derive(Debug, Clone)]
pub struct BatchResponse {
    json: String,
}

/// Create a JSON-RPC error response.
pub fn batch_response_error(id: Id, err: impl Into<ErrorObject<'static>>) -> String {
    let err = ResponsePayload::<()>::error_borrowed(err);
    serde_json::to_string(&Response::new(err, id)).expect("JSON serialization infallible; qed")
}

/// Size of a method's last response.
///
/// Responses of the same method tend to have similar sizes, so serializing into a buffer of the
/// previous size avoids growing (and copying) large responses step by step.
#[derive(Debug)]
pub(crate) struct ResponseSizeHint(AtomicUsize);

impl ResponseSizeHint {
    pub(crate) const fn new() -> Self {
        Self(AtomicUsize::new(128))
    }

    fn get(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    fn record(&self, len: usize) {
        self.0.store(len.min(MAX_RESPONSE_SIZE_HINT), Ordering::Relaxed);
    }
}

/// Largest [`ResponseSizeHint`], so one huge response doesn't make every later call of the method,
/// from any connection, reserve as much.
const MAX_RESPONSE_SIZE_HINT: usize = 8 * 1024 * 1024;

/// Bounded writer that allows writing at most `max_len` bytes.
#[derive(Debug, Clone)]
struct BoundedWriter {
    max_len: usize,
    buf: Vec<u8>,
}

impl BoundedWriter {
    /// Create a new bounded writer with room for `capacity` bytes.
    pub fn new(max_len: usize, capacity: usize) -> Self {
        Self { max_len, buf: Vec::with_capacity(capacity.min(max_len)) }
    }

    /// Consume the writer and extract the written bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

// serde_json emits a response as many small writes, so both methods are inlined into the
// serializer and only check the limit, instead of going through the default `write_all` loop.
impl io::Write for &mut BoundedWriter {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all(buf)?;
        Ok(buf.len())
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        if buf.len() > self.max_len - self.buf.len() {
            return Err(limit_exceeded());
        }
        self.buf.extend_from_slice(buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cold]
fn limit_exceeded() -> io::Error {
    io::Error::new(io::ErrorKind::OutOfMemory, "Memory capacity exceeded")
}

#[cfg(test)]
mod tests {
    use super::{
        BatchResponseBuilder, BoundedWriter, Id, MethodResponse, ResponsePayload, ResponseSizeHint,
        MAX_RESPONSE_SIZE_HINT,
    };

    #[test]
    fn batch_with_single_works() {
        let method = MethodResponse::response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&"a"),
            usize::MAX,
        );
        assert_eq!(method.json.len(), 37);

        // Recall a batch appends two bytes for the `[]`.
        let mut builder = BatchResponseBuilder::new_with_limit(39);
        builder.append(method).unwrap();
        let batch = builder.finish();

        assert_eq!(batch.json, r#"[{"jsonrpc":"2.0","id":1,"result":"a"}]"#)
    }

    #[test]
    fn batch_with_multiple_works() {
        let m1 = MethodResponse::response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&"a"),
            usize::MAX,
        );
        let m11 = MethodResponse::response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&"a"),
            usize::MAX,
        );
        assert_eq!(m1.json.len(), 37);

        // Recall a batch appends two bytes for the `[]` and one byte for `,` to append a method
        // call. so it should be 2 + (37 * n) + (n-1)
        let limit = 2 + (37 * 2) + 1;
        let mut builder = BatchResponseBuilder::new_with_limit(limit);
        builder.append(m1).unwrap();
        builder.append(m11).unwrap();
        let batch = builder.finish();

        assert_eq!(
            batch.json,
            r#"[{"jsonrpc":"2.0","id":1,"result":"a"},{"jsonrpc":"2.0","id":1,"result":"a"}]"#
        )
    }

    #[test]
    fn batch_empty_err() {
        let batch = BatchResponseBuilder::new_with_limit(1024).finish();

        let exp_err =
            r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}}"#;
        assert_eq!(batch.json, exp_err);
    }

    #[test]
    fn batch_too_big() {
        let method = MethodResponse::response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&"a".repeat(28)),
            128,
        );
        assert_eq!(method.json.len(), 64);

        let batch = BatchResponseBuilder::new_with_limit(63).append(method).unwrap_err();

        let exp_err = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32011,"message":"The batch response was too large","data":"Exceeded max limit of 63"}}"#;
        assert_eq!(batch.json, exp_err);
    }

    #[test]
    fn bounded_serializer_work() {
        use jsonrpsee_types::{Response, ResponsePayload};

        let mut writer = BoundedWriter::new(100, 128);
        let result = ResponsePayload::success(&"success");
        let rp = &Response::new(result, Id::Number(1));

        assert!(serde_json::to_writer(&mut writer, rp).is_ok());
        assert_eq!(
            String::from_utf8(writer.into_bytes()).unwrap(),
            r#"{"jsonrpc":"2.0","id":1,"result":"success"}"#
        );
    }

    #[test]
    fn bounded_serializer_cap_works() {
        let mut writer = BoundedWriter::new(100, 128);
        // NOTE: `"` is part of the serialization so 101 characters.
        assert!(serde_json::to_writer(&mut writer, &"x".repeat(99)).is_err());
    }

    #[test]
    fn size_hint_is_capped_and_small_responses_shrink() {
        let hint = ResponseSizeHint::new();
        let big = "x".repeat(MAX_RESPONSE_SIZE_HINT);
        let rp = MethodResponse::sized_response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&big),
            usize::MAX,
            &hint,
        );
        assert!(rp.json.len() > MAX_RESPONSE_SIZE_HINT);
        assert_eq!(hint.get(), MAX_RESPONSE_SIZE_HINT);

        let rp = MethodResponse::sized_response(
            Id::Number(1),
            ResponsePayload::success_borrowed(&"a"),
            usize::MAX,
            &hint,
        );
        assert_eq!(rp.json, r#"{"jsonrpc":"2.0","id":1,"result":"a"}"#);
        assert!(rp.json.capacity() < 2 * rp.json.len());
        assert_eq!(hint.get(), rp.json.len());
    }
}
