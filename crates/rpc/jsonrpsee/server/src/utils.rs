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

use std::{
    borrow::Cow,
    future::Future,
    io,
    pin::{pin, Pin},
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use crate::{ConnectionPermit, HttpBody};

use futures_util::{future, ready};
use hyper::body::{Bytes, Frame};
use hyper_util::rt::{TokioExecutor, TokioIo};
use jsonrpsee_core::{middleware::Notification, BoxError, JsonRawValue};
use jsonrpsee_types::TwoPointZero;
use pin_project::pin_project;
use serde::{Deserialize, Deserializer};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::Notify,
    time::Sleep,
};

/// Largest data frame [`PermitBody`] yields.
const BODY_CHUNK_SIZE: usize = 64 * 1024;

/// Default for how long a connection may take to send its first request.
pub(crate) const DEFAULT_FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an accept loop pauses after `accept` fails for a reason that outlasts the attempt.
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_secs(1);

/// Default timeout for writing a single WebSocket message and for a stalled HTTP response write,
/// matching geth.
pub(crate) const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Notifies `request_received` whenever hyper passes a request to the service.
#[derive(Debug, Clone)]
pub(crate) struct NotifyOnRequest<S> {
    service: S,
    request_received: Arc<Notify>,
}

impl<S> NotifyOnRequest<S> {
    pub(crate) fn new(service: S, request_received: Arc<Notify>) -> Self {
        Self { service, request_received }
    }
}

impl<S, R> hyper::service::Service<R> for NotifyOnRequest<S>
where
    S: hyper::service::Service<R>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn call(&self, request: R) -> Self::Future {
        self.request_received.notify_one();
        self.service.call(request)
    }
}

/// Resolves with an error if no request was received within `timeout`, otherwise never.
pub(crate) async fn no_request_within(
    request_received: &Notify,
    timeout: Option<Duration>,
) -> BoxError {
    if let Some(timeout) = timeout &&
        tokio::time::timeout(timeout, request_received.notified()).await.is_err()
    {
        return io::Error::new(io::ErrorKind::TimedOut, "no request received in time").into();
    }

    future::pending().await
}

/// Serve a service over a TCP connection with graceful shutdown.
/// This means that pending requests will be completed before the server is stopped.
///
/// Connections which don't send a request within 30 seconds are closed.
pub async fn serve_with_graceful_shutdown<S, B, I>(
    io: I,
    service: S,
    stopped: impl Future<Output = ()>,
) -> Result<(), BoxError>
where
    S: tower::Service<http::Request<hyper::body::Incoming>, Response = http::Response<B>>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
    S::Response: Send,
    S::Error: Into<BoxError>,
    B: http_body::Body<Data = hyper::body::Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let request_received = Arc::new(Notify::new());
    let service = NotifyOnRequest::new(
        hyper_util::service::TowerToHyperService::new(service),
        request_received.clone(),
    );
    let io = TokioIo::new(io);

    let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(io, service);

    tokio::pin!(stopped, conn);

    tokio::select! {
        // Return if the connection was completed.
        res = &mut conn => res,
        // If the server is stopped, we should gracefully shutdown
        // the connection and poll it until it finishes.
        _ = &mut stopped => {
            conn.as_mut().graceful_shutdown();
            conn.await
        }
        err = no_request_within(&request_received, Some(DEFAULT_FIRST_REQUEST_TIMEOUT)) => Err(err),
    }
}

/// Deserializes a JSON-RPC notification, rejecting objects that contain an `id` member.
pub(crate) fn deserialize_notification(data: &[u8]) -> Result<Notification<'_>, serde_json::Error> {
    let notif = serde_json::from_slice::<NotificationWithoutId<'_>>(data)?;
    Ok(Notification::new(notif.method, notif.params.map(Cow::Borrowed)))
}

#[derive(Deserialize)]
struct NotificationWithoutId<'a> {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: TwoPointZero,
    #[serde(borrow)]
    method: Cow<'a, str>,
    #[serde(borrow)]
    params: Option<&'a JsonRawValue>,
    #[serde(default, rename = "id", deserialize_with = "reject_notification_id")]
    _id: (),
}

fn reject_notification_id<'de, D: Deserializer<'de>>(_: D) -> Result<(), D::Error> {
    Err(serde::de::Error::custom("notification must not contain an id"))
}

/// Pauses an accept loop after `accept` failed with `err`.
///
/// Errors that concern only the connection being accepted (refused, aborted or reset) return at
/// once. Any other error, such as running out of file descriptors, persists and would make the
/// loop spin, so it is followed by a one second sleep.
///
/// Returns the `stopped` future to keep polling, or `None` if it resolved during the sleep and
/// the loop must exit.
pub async fn accept_error_backoff<S>(err: &io::Error, stopped: S) -> Option<S>
where
    S: Future + Unpin,
{
    if matches!(
        err.kind(),
        io::ErrorKind::ConnectionRefused |
            io::ErrorKind::ConnectionAborted |
            io::ErrorKind::ConnectionReset
    ) {
        return Some(stopped);
    }

    match future::select(pin!(tokio::time::sleep(ACCEPT_ERROR_BACKOFF)), stopped).await {
        future::Either::Left((_, stopped)) => Some(stopped),
        future::Either::Right(_) => None,
    }
}

/// HTTP response body that holds the connection permit until the body is written or dropped.
///
/// hyper writes the body after the service has returned the response, so a permit dropped by the
/// service would no longer limit the connections that are still sending a response.
///
/// hyper drops the body as soon as it has taken the last frame, even if it still has to write
/// that frame to the socket. A response is therefore yielded in frames of at most
/// [`BODY_CHUNK_SIZE`] bytes: hyper only takes the next frame while its write buffer has room, so
/// the permit is held until all but the last few frames have been written.
#[pin_project]
pub(crate) struct PermitBody {
    #[pin]
    inner: HttpBody,
    /// Rest of the data frame that is being yielded in chunks.
    pending: Bytes,
    _conn_permit: ConnectionPermit,
}

impl PermitBody {
    pub(crate) fn new(inner: HttpBody, conn_permit: ConnectionPermit) -> Self {
        Self { inner, pending: Bytes::new(), _conn_permit: conn_permit }
    }
}

impl http_body::Body for PermitBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();

        if this.pending.is_empty() {
            match ready!(this.inner.poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => *this.pending = data,
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                end_or_error => return Poll::Ready(end_or_error),
            }
        }

        let chunk = this.pending.split_to(this.pending.len().min(BODY_CHUNK_SIZE));
        Poll::Ready(Some(Ok(Frame::data(chunk))))
    }

    fn is_end_stream(&self) -> bool {
        self.pending.is_empty() && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        let pending = self.pending.len() as u64;
        let mut hint = self.inner.size_hint();
        hint.set_lower(hint.lower() + pending);
        if let Some(upper) = hint.upper() {
            hint.set_upper(upper + pending);
        }
        hint
    }
}

/// IO that fails a write with [`io::ErrorKind::TimedOut`] once the peer has accepted no bytes for
/// the configured duration.
///
/// A peer that stops reading leaves hyper's buffered response, and the permit held by its body,
/// pending forever otherwise.
#[pin_project]
pub(crate) struct WriteTimeoutIo<T> {
    #[pin]
    inner: T,
    timeout: Duration,
    /// Armed when a write returns `Pending` and cleared when a write makes progress.
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<T> WriteTimeoutIo<T> {
    pub(crate) fn new(inner: T, timeout: Duration) -> Self {
        Self { inner, timeout, stalled: None }
    }
}

impl<T: AsyncRead> AsyncRead for WriteTimeoutIo<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.project().inner.poll_read(cx, buf)
    }
}

impl<T: AsyncWrite> AsyncWrite for WriteTimeoutIo<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let poll = this.inner.poll_write(cx, buf);
        poll_write_timeout(this.stalled, *this.timeout, cx, poll)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let poll = this.inner.poll_write_vectored(cx, bufs);
        poll_write_timeout(this.stalled, *this.timeout, cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().inner.poll_shutdown(cx)
    }
}

/// Arms the stall timer when a write is `Pending`, so the task is woken when it fires, and clears
/// it when the write makes progress.
fn poll_write_timeout(
    stalled: &mut Option<Pin<Box<Sleep>>>,
    timeout: Duration,
    cx: &mut Context<'_>,
    poll: Poll<io::Result<usize>>,
) -> Poll<io::Result<usize>> {
    if poll.is_ready() {
        *stalled = None;
        return poll;
    }

    let stalled = stalled.get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
    match stalled.as_mut().poll(cx) {
        Poll::Ready(()) => Poll::Ready(Err(io::ErrorKind::TimedOut.into())),
        Poll::Pending => Poll::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[test]
    fn notification_id_classification_notification_metadata() {
        for (raw, expected) in [
            (
                r#"{"jsonrpc":"2.0","method":"example","params":[1],"extra":{"id":true}}"#,
                Some("[1]"),
            ),
            (r#"{"jsonrpc":"2.0","method":"example"}"#, None),
        ] {
            let notification = deserialize_notification(raw.as_bytes()).unwrap();
            assert_eq!(notification.method_name(), "example");
            assert_eq!(notification.params().as_ref().map(|p| p.get()), expected);
            assert!(!matches!(notification.params(), Some(Cow::Owned(_))));
        }
        for id in ["true", "false", "{}", "[]", "null", "0", "\"valid\""] {
            let raw = format!(r#"{{"jsonrpc":"2.0","method":"example","id":{id}}}"#);
            assert!(deserialize_notification(raw.as_bytes()).is_err());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn accept_error_backoff_skips_connection_errors() {
        for kind in [
            io::ErrorKind::ConnectionRefused,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
        ] {
            let start = Instant::now();
            let stopped =
                accept_error_backoff(&io::Error::from(kind), future::pending::<()>()).await;
            assert!(stopped.is_some());
            assert_eq!(start.elapsed(), Duration::ZERO);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn accept_error_backoff_sleeps_on_persistent_errors() {
        let start = Instant::now();
        let err = io::Error::other("too many open files");
        assert!(accept_error_backoff(&err, future::pending::<()>()).await.is_some());
        assert_eq!(start.elapsed(), ACCEPT_ERROR_BACKOFF);
    }

    #[tokio::test(start_paused = true)]
    async fn accept_error_backoff_observes_stop() {
        let start = Instant::now();
        let err = io::Error::other("out of descriptors");
        assert!(accept_error_backoff(&err, future::ready(())).await.is_none());
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}
