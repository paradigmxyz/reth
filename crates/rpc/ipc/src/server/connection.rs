//! A IPC connection.

use crate::stream_codec::StreamCodec;
use futures::{stream::FuturesUnordered, FutureExt, Sink, Stream};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;
use tower::Service;

pub(crate) type JsonRpcStream<T> = Framed<T, StreamCodec>;

#[pin_project::pin_project]
pub(crate) struct IpcConn<T>(#[pin] pub(crate) T);

impl<T> Stream for IpcConn<JsonRpcStream<T>>
where
    T: AsyncRead + AsyncWrite,
{
    type Item = io::Result<String>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().0.poll_next(cx)
    }
}

impl<T> Sink<String> for IpcConn<JsonRpcStream<T>>
where
    T: AsyncRead + AsyncWrite,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // NOTE: we always flush here this prevents buffering in the underlying
        // `Framed` impl that would cause stalled requests
        self.project().0.poll_flush(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: String) -> Result<(), Self::Error> {
        self.project().0.start_send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().0.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().0.poll_close(cx)
    }
}

/// Drives an [IpcConn] forward.
///
/// This forwards received requests from the connection to the service and sends responses to the
/// connection.
///
/// This future terminates when the connection is closed.
#[pin_project::pin_project]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct IpcConnDriver<T, S, Fut> {
    #[pin]
    pub(crate) conn: IpcConn<JsonRpcStream<T>>,
    pub(crate) service: S,
    /// rpc requests in progress
    #[pin]
    pub(crate) pending_calls: FuturesUnordered<Fut>,
    pub(crate) items: VecDeque<String>,
}

impl<T, S, Fut> IpcConnDriver<T, S, Fut> {
    /// Add a new item to the send queue.
    pub(crate) fn push_back(&mut self, item: String) {
        self.items.push_back(item);
    }
}

impl<T, S> Future for IpcConnDriver<T, S, S::Future>
where
    S: Service<String, Response = Option<String>> + Send + 'static,
    S::Error: Into<Box<dyn core::error::Error + Send + Sync>>,
    S::Future: Send + Unpin,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // items are also pushed from external
        // this will act as a manual yield point to reduce latencies of the polling future that may
        // submit items from an additional source (subscription)
        let mut budget = 5;

        // ensure we still have enough budget for another iteration
        'outer: loop {
            budget -= 1;
            if budget == 0 {
                // make sure we're woken up again
                cx.waker().wake_by_ref();
                return Poll::Pending
            }

            // write all responses to the sink
            while this.conn.as_mut().poll_ready(cx).is_ready() {
                if let Some(item) = this.items.pop_front() {
                    if let Err(err) = this.conn.as_mut().start_send(item) {
                        tracing::warn!("IPC response failed: {:?}", err);
                        return Poll::Ready(())
                    }
                } else {
                    break
                }
            }

            'inner: loop {
                // drain all calls that are ready and put them in the output item queue
                let drained = if this.pending_calls.is_empty() {
                    false
                } else {
                    if let Poll::Ready(Some(res)) = this.pending_calls.as_mut().poll_next(cx) {
                        let item = match res {
                            Ok(Some(resp)) => resp,
                            Ok(None) => continue 'inner,
                            Err(err) => err.into().to_string(),
                        };
                        this.items.push_back(item);
                        continue 'outer;
                    }
                    true
                };

                // read from the stream
                match this.conn.as_mut().poll_next(cx) {
                    Poll::Ready(res) => match res {
                        Some(Ok(item)) => {
                            let mut call = this.service.call(item);
                            match call.poll_unpin(cx) {
                                Poll::Ready(res) => {
                                    let item = match res {
                                        Ok(Some(resp)) => resp,
                                        Ok(None) => continue 'inner,
                                        Err(err) => err.into().to_string(),
                                    };
                                    this.items.push_back(item);
                                    continue 'outer
                                }
                                Poll::Pending => {
                                    this.pending_calls.push(call);
                                }
                            }
                        }
                        Some(Err(err)) => {
                            // this can happen if the client closes the connection
                            tracing::debug!("IPC request failed: {:?}", err);
                            return Poll::Ready(())
                        }
                        None => return Poll::Ready(()),
                    },
                    Poll::Pending => {
                        if drained || this.pending_calls.is_empty() {
                            // at this point all things are pending
                            return Poll::Pending
                        }
                    }
                }
            }
        }
    }
}
