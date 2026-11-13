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
    future::Future,
    io,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{ready, Context, Poll},
};

use bytes::Bytes;
use futures_util::{future::BoxFuture, FutureExt, Stream, StreamExt};
use jsonrpsee_types::{
    error::{reject_too_big_batch_response, reject_too_big_response, ErrorCode, ErrorObject},
    ErrorObjectOwned, Id, Response,
};
use serde::Serialize;
use tokio::sync::mpsc;

pub use jsonrpsee_types::ResponsePayload;

#[derive(Debug)]
enum ResponseKind {
    MethodCall,
    /// A method call whose result is still being produced, see [`ResponseStream`]. Boxed, so it
    /// doesn't grow every other response.
    Stream(Box<StreamedJson>),
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

    /// Returns whether the response is a method response, streamed or not.
    pub fn is_method_call(&self) -> bool {
        matches!(self.kind, ResponseKind::MethodCall | ResponseKind::Stream(_))
    }

    /// Returns whether the response is a batch response.
    pub fn is_batch(&self) -> bool {
        matches!(self.kind, ResponseKind::Batch)
    }

    /// Consume the method response and extract the serialized JSON response.
    ///
    /// # Panics
    ///
    /// If the response is a stream, which must be [buffered](MethodResponse::buffer) first.
    pub fn into_json(self) -> String {
        assert!(
            !matches!(self.kind, ResponseKind::Stream(_)),
            "a streamed response must be buffered first"
        );
        self.json
    }

    /// Consume the method response and return its body, whole or as a stream of parts.
    pub fn into_body(self) -> ResponseBody {
        match self.kind {
            ResponseKind::Stream(stream) => ResponseBody::Stream(*stream),
            _ => ResponseBody::Json(self.json),
        }
    }

    /// Waits for the rest of a streamed response and returns it as a whole. Other responses are
    /// returned unchanged.
    ///
    /// If the result fails or exceeds the response size limit after it started, the response is
    /// replaced with that error, because none of it was sent yet.
    pub async fn buffer(self) -> Self {
        let ResponseKind::Stream(mut stream) = self.kind else { return self };
        let mut json = Vec::with_capacity(stream.head.len());
        while let Some(part) = stream.next().await {
            match part {
                Ok(part) => json.extend_from_slice(&part),
                Err(err) => return Self::error(stream.id, err),
            }
        }
        let Ok(json) = String::from_utf8(json) else {
            tracing::error!(target: LOG_TARGET, "Streamed response is not valid UTF-8");
            return Self::error(stream.id, ErrorCode::InternalError)
        };
        Self { json, success_or_error: self.success_or_error, kind: ResponseKind::MethodCall }
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
    /// previous responses and records the size of this one if it is a success.
    pub(crate) fn sized_response<T>(
        id: Id,
        rp: ResponsePayload<T>,
        max_response_size: usize,
        size_hint: &ResponseSizeHint,
    ) -> Self
    where
        T: Serialize + Clone,
    {
        // An error's size says nothing about the method's results, so it neither uses nor updates
        // the hint.
        if matches!(rp, ResponsePayload::Error(_)) {
            return Self::response(id, rp, max_response_size)
        }
        let mut rp = Self::response_with_capacity(id, rp, max_response_size, size_hint.capacity());
        // A response much smaller than the hint would otherwise keep the unused capacity until it
        // is sent.
        let unused = rp.json.capacity() - rp.json.len();
        if unused > rp.json.len() && unused > RESPONSE_SHRINK_THRESHOLD {
            rp.json.shrink_to_fit();
        }
        // A success that exceeded the limit was replaced with an error, which isn't recorded.
        if rp.is_success() {
            size_hint.record(rp.json.len());
        }
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
                    Self::error(id, reject_too_big_response(max_response_size))
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

    /// Create a response whose result is produced in parts by `stream`.
    ///
    /// Waits for the first part, so a result that fails before it produced anything is answered
    /// with a regular JSON-RPC error.
    pub(crate) async fn stream(
        id: Id<'static>,
        stream: ResponseStream,
        max_response_size: usize,
    ) -> Self {
        let mut json = String::from(r#"{"jsonrpc":"2.0","id":"#);
        // SAFETY: serde_json does not emit invalid UTF-8.
        serde_json::to_writer(unsafe { json.as_mut_vec() }, &id)
            .expect("JSON serialization infallible; qed");
        json.push_str(r#","result":"#);

        let Some(remaining) = max_response_size.checked_sub(json.len()) else {
            return Self::error(id, reject_too_big_response(max_response_size))
        };
        let ResponseStream { parts, producer } = stream;
        let mut stream = StreamedJson {
            id,
            head: Bytes::new(),
            parts,
            producer: Some(producer),
            outcome: Ok(()),
            remaining,
            max_response_size,
            done: false,
        };
        let first = match stream.next().await {
            // A producer that finished without a part yields the closing brace and is done, but a
            // result needs at least one part to be a JSON value.
            Some(Ok(first)) if !stream.done => first,
            Some(Err(err)) => return Self::error(stream.id, err),
            _ => return Self::error(stream.id, ErrorCode::InternalError),
        };
        stream.head = Bytes::from([json.as_bytes(), &first].concat());

        Self {
            json,
            success_or_error: MethodResponseResult::Success,
            kind: ResponseKind::Stream(Box::new(stream)),
        }
    }
}

/// Serialized response of a method call, see [`MethodResponse::into_body`].
#[derive(Debug)]
pub enum ResponseBody {
    /// The whole response.
    Json(String),
    /// A response whose result is still being produced.
    Stream(StreamedJson),
}

/// A response that is serialized while its result is produced.
///
/// It drives the producer of the [`ResponseStream`] and yields the response in parts that together
/// form one JSON-RPC response. Once the result fails or exceeds the response size limit, it yields
/// an error and ends. Parts that were already sent can't be taken back, so the transport has to
/// abort the response instead of completing it. Dropping it drops the producer.
pub struct StreamedJson {
    id: Id<'static>,
    /// Envelope and first part of the result, yielded first.
    head: Bytes,
    parts: mpsc::Receiver<Bytes>,
    /// `None` once the producer finished with `outcome`.
    producer: Option<BoxFuture<'static, Result<(), ErrorObjectOwned>>>,
    outcome: Result<(), ErrorObjectOwned>,
    /// Bytes left before the response exceeds `max_response_size`.
    remaining: usize,
    max_response_size: usize,
    done: bool,
}

impl std::fmt::Debug for StreamedJson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedJson").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Stream for StreamedJson {
    type Item = Result<Bytes, ErrorObjectOwned>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None)
        }
        if !this.head.is_empty() {
            return Poll::Ready(Some(Ok(std::mem::take(&mut this.head))))
        }

        let part = loop {
            if let Poll::Ready(Some(part)) = this.parts.poll_recv(cx) {
                break Ok(part)
            }
            let Some(producer) = &mut this.producer else {
                // The producer finished and every part it sent before is taken.
                this.done = true;
                break std::mem::replace(&mut this.outcome, Ok(()))
                    .map(|()| Bytes::from_static(b"}"))
            };
            this.outcome = ready!(producer.as_mut().poll(cx));
            this.producer = None;
        };
        let part = part.and_then(|part| match this.remaining.checked_sub(part.len()) {
            Some(remaining) => {
                this.remaining = remaining;
                Ok(part)
            }
            None => Err(reject_too_big_response(this.max_response_size)),
        });
        if part.is_err() {
            this.done = true;
        }
        Poll::Ready(Some(part))
    }
}

/// A method result that is produced in parts.
///
/// Return it from a method registered with
/// [`RpcModule::register_streaming_method`](crate::server::RpcModule::register_streaming_method).
pub struct ResponseStream {
    parts: mpsc::Receiver<Bytes>,
    producer: BoxFuture<'static, Result<(), ErrorObjectOwned>>,
}

impl ResponseStream {
    /// Creates a result that `producer` sends in parts through the given sender.
    ///
    /// The parts are concatenated as they are, so together they must form the result as one JSON
    /// value. Sending waits while the transport has not taken the previous parts yet, and fails
    /// once the response was dropped, so the producer can stop. The transport polls the future
    /// while it sends the response and drops it with the response. The result is complete once
    /// the future returns `Ok` and fails with the error it returns otherwise.
    pub fn new<F, Fut, E>(producer: F) -> Self
    where
        F: FnOnce(mpsc::Sender<Bytes>) -> Fut,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: Into<ErrorObjectOwned>,
    {
        let (tx, parts) = mpsc::channel(RESPONSE_STREAM_CAPACITY);
        let producer = producer(tx);
        Self { parts, producer: async move { producer.await.map_err(Into::into) }.boxed() }
    }
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseStream").finish_non_exhaustive()
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

/// Builder for the response to a batch request.
#[derive(Debug)]
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

    /// Finish the batch response.
    ///
    /// An empty batch is answered with an invalid request error.
    pub fn finish(mut self) -> MethodResponse {
        if self.is_empty() {
            return MethodResponse::error(Id::Null, ErrorObject::from(ErrorCode::InvalidRequest))
        }
        // Replace the trailing `,` of the last entry.
        self.result.pop();
        self.result.push(']');
        MethodResponse {
            json: self.result,
            success_or_error: MethodResponseResult::Success,
            kind: ResponseKind::Batch,
        }
    }
}

/// Parts of a streamed result that wait for the transport, so a producer runs at most this far
/// ahead of a slow client while it never has to wait for the transport to take a part.
const RESPONSE_STREAM_CAPACITY: usize = 4;

/// Expected size of a method's next successful response.
///
/// Responses of the same method tend to have similar sizes, so serializing into a buffer of the
/// previous size avoids growing (and copying) large responses step by step. The hint is a decaying
/// maximum of the method's successful responses, so a `null` or a short response between large
/// ones barely lowers it, and errors don't change it.
#[derive(Debug)]
pub(crate) struct ResponseSizeHint(AtomicUsize);

impl ResponseSizeHint {
    pub(crate) const fn new() -> Self {
        Self(AtomicUsize::new(128))
    }

    /// Returns the capacity to reserve for the next response.
    ///
    /// It has a quarter more room than the hint, so a response slightly larger than the previous
    /// ones doesn't double the buffer.
    fn capacity(&self) -> usize {
        let hint = self.0.load(Ordering::Relaxed);
        (hint + hint / 4).min(MAX_RESPONSE_SIZE_HINT)
    }

    /// Records the size of a successful response.
    fn record(&self, len: usize) {
        // Concurrent calls may overwrite each other's update. The hint is only an estimate, so a
        // lost update costs at most one regrown buffer.
        let hint = self.0.load(Ordering::Relaxed);
        let decayed = hint - hint / 8;
        self.0.store(len.max(decayed).min(MAX_RESPONSE_SIZE_HINT), Ordering::Relaxed);
    }
}

/// Largest capacity a [`ResponseSizeHint`] reserves, so one huge response doesn't make every later
/// call of the method, from any connection, reserve as much.
const MAX_RESPONSE_SIZE_HINT: usize = 8 * 1024 * 1024;

/// A response is shrunk only if more than half of its buffer, and more than this many bytes, is
/// unused. Smaller buffers aren't worth reallocating.
const RESPONSE_SHRINK_THRESHOLD: usize = 16 * 1024;

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
    use super::*;
    use jsonrpsee_types::error::OVERSIZED_RESPONSE_CODE;

    /// Streams `parts`, which then finish with `outcome`, as the result of a call with id 1.
    async fn stream(
        parts: &'static [&'static str],
        outcome: Result<(), ErrorObjectOwned>,
        max_response_size: usize,
    ) -> MethodResponse {
        let stream = ResponseStream::new(move |tx| async move {
            for part in parts {
                tx.send(Bytes::from_static(part.as_bytes())).await.unwrap();
            }
            outcome
        });
        MethodResponse::stream(Id::Number(1), stream, max_response_size).await
    }

    #[tokio::test]
    async fn stream_buffers_parts_and_errors() {
        let ok = r#"{"jsonrpc":"2.0","id":1,"result":[1,2]}"#;
        let custom = ErrorObject::owned(1, "custom", None::<()>);
        let failed = r#"{"jsonrpc":"2.0","id":1,"error":{"code":1,"message":"custom"}}"#;

        let rp = stream(&["[1", ",2", "]"], Ok(()), usize::MAX).await;
        assert_eq!(rp.buffer().await.into_json(), ok);
        // Fails before the first part, after it, and without any part.
        let rp = stream(&[], Err(custom.clone()), usize::MAX).await;
        assert_eq!(rp.into_json(), failed);
        let rp = stream(&["[1"], Err(custom.clone()), usize::MAX).await;
        assert_eq!(rp.buffer().await.into_json(), failed);
        let rp = stream(&[], Ok(()), usize::MAX).await;
        assert_eq!(rp.as_error_code(), Some(ErrorCode::InternalError.code()));

        // Sent in parts, a failure after the first part ends the stream with the error.
        let rp = stream(&["[1"], Err(custom.clone()), usize::MAX).await;
        let ResponseBody::Stream(mut body) = rp.into_body() else { panic!("not a stream") };
        assert_eq!(body.next().await.unwrap().unwrap(), r#"{"jsonrpc":"2.0","id":1,"result":[1"#);
        assert_eq!(body.next().await.unwrap().unwrap_err(), custom);
        assert!(body.next().await.is_none());

        // The closing brace, a later part, and the first part each exceed the limit.
        let rp = stream(&["[1", ",2", "]"], Ok(()), ok.len()).await;
        assert_eq!(rp.buffer().await.into_json(), ok);
        for limit in [ok.len() - 1, ok.len() - 2, ok.len() - 4] {
            let rp = stream(&["[1", ",2", "]"], Ok(()), limit).await;
            assert_eq!(rp.buffer().await.as_error_code(), Some(OVERSIZED_RESPONSE_CODE));
        }
    }

    #[tokio::test]
    async fn dropped_stream_closes_sender() {
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let stream = ResponseStream::new(move |tx| {
            std::thread::spawn(move || {
                while tx.blocking_send(Bytes::from_static(b"0")).is_ok() {}
                closed_tx.send(()).unwrap();
            });
            std::future::pending::<Result<(), ErrorObjectOwned>>()
        });
        drop(MethodResponse::stream(Id::Number(1), stream, usize::MAX).await);
        closed_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    }

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

    /// Serializes `rp` like a method registered on a module.
    fn sized_response<T: Serialize + Clone>(
        rp: ResponsePayload<'_, T>,
        hint: &ResponseSizeHint,
    ) -> MethodResponse {
        MethodResponse::sized_response(Id::Number(1), rp, usize::MAX, hint)
    }

    fn success<T: Clone>(result: &T) -> ResponsePayload<'_, T> {
        ResponsePayload::success_borrowed(result)
    }

    #[test]
    fn size_hint_is_capped_and_small_responses_shrink() {
        let hint = ResponseSizeHint::new();
        let big = "x".repeat(MAX_RESPONSE_SIZE_HINT);
        let rp = sized_response(success(&big), &hint);
        assert!(rp.json.len() > MAX_RESPONSE_SIZE_HINT);
        assert_eq!(hint.0.load(Ordering::Relaxed), MAX_RESPONSE_SIZE_HINT);
        assert_eq!(hint.capacity(), MAX_RESPONSE_SIZE_HINT);

        let rp = sized_response(success(&"a"), &hint);
        assert_eq!(rp.json, r#"{"jsonrpc":"2.0","id":1,"result":"a"}"#);
        assert!(rp.json.capacity() < 2 * rp.json.len());
    }

    #[test]
    fn size_hint_survives_errors_and_null() {
        let hint = ResponseSizeHint::new();
        let big = "x".repeat(100_000);
        let big_len = sized_response(success(&big), &hint).json.len();

        let rp = sized_response(ResponsePayload::<()>::error(ErrorCode::InternalError), &hint);
        assert_eq!(rp.as_error_code(), Some(ErrorCode::InternalError.code()));
        let rp = sized_response(success(&None::<u64>), &hint);
        assert_eq!(rp.json, r#"{"jsonrpc":"2.0","id":1,"result":null}"#);

        let reserved = hint.capacity();
        assert!(reserved >= big_len, "reserved {reserved} for {big_len} bytes");
        let rp = sized_response(success(&big), &hint);
        // Growing the buffer would at least double it.
        assert!(rp.json.capacity() < 2 * reserved);
    }

    #[test]
    fn errors_do_not_reserve_the_hint() {
        let hint = ResponseSizeHint::new();
        sized_response(success(&"x".repeat(8 * 1024)), &hint);

        let rp = sized_response(ResponsePayload::<()>::error(ErrorCode::InternalError), &hint);
        assert!(rp.json.capacity() < hint.capacity());
    }

    #[test]
    fn oversized_response_does_not_change_the_hint() {
        let hint = ResponseSizeHint::new();
        let rp =
            MethodResponse::sized_response(Id::Number(1), success(&"x".repeat(100)), 64, &hint);
        assert_eq!(rp.as_error_code(), Some(OVERSIZED_RESPONSE_CODE));
        assert_eq!(
            hint.0.load(Ordering::Relaxed),
            ResponseSizeHint::new().0.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn slightly_larger_response_fits_the_reserved_buffer() {
        let hint = ResponseSizeHint::new();
        sized_response(success(&"x".repeat(100_000)), &hint);

        let reserved = hint.capacity();
        let rp = sized_response(success(&"x".repeat(110_000)), &hint);
        // Growing the buffer would at least double it.
        assert!(rp.json.capacity() < 2 * reserved);
    }

    #[test]
    fn size_hint_decays_to_smaller_responses() {
        let hint = ResponseSizeHint::new();
        sized_response(success(&"x".repeat(100_000)), &hint);

        let small = sized_response(success(&"a"), &hint).json.len();
        assert!(hint.0.load(Ordering::Relaxed) > 80_000);
        for _ in 0..100 {
            sized_response(success(&"a"), &hint);
        }
        assert_eq!(hint.0.load(Ordering::Relaxed), small);
    }

    #[test]
    fn small_unused_capacity_is_not_shrunk() {
        let hint = ResponseSizeHint::new();
        sized_response(success(&"x".repeat(8 * 1024)), &hint);

        let reserved = hint.capacity();
        let rp = sized_response(success(&"a"), &hint);
        // Most of the buffer is unused, but too little to be worth reallocating.
        assert!(rp.json.capacity() >= reserved);
    }
}
