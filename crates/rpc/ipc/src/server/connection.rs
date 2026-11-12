//! An IPC connection.

use super::MESSAGE_BUFFER_CAPACITY;
use crate::stream_codec::StreamCodec;
use futures::{stream::FuturesUnordered, FutureExt, Sink, Stream};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};
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

/// Drives an [`IpcConn`] forward.
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
    /// Messages sent outside of calls, such as subscription notifications.
    pub(crate) messages: mpsc::Receiver<String>,
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

            // Only take messages while the send queue has room, so a client that stops reading
            // backs up its subscriptions instead of growing the queue.
            let mut messages_pending = false;
            while this.items.len() < MESSAGE_BUFFER_CAPACITY {
                let Poll::Ready(Some(item)) = this.messages.poll_recv(cx) else {
                    messages_pending = true;
                    break
                };
                this.items.push_back(item);
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

            // A full queue left `messages` unpolled, so it won't wake this task for the messages
            // that are still waiting. Take them now that the writes made room.
            if !messages_pending && this.items.len() < MESSAGE_BUFFER_CAPACITY {
                continue 'outer
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

                // The queue is only full if the socket isn't writable, which wakes this task once
                // it is. Until then, new requests stay unread.
                if this.items.len() >= MESSAGE_BUFFER_CAPACITY {
                    return Poll::Pending
                }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::pin,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        task::{Wake, Waker},
    };
    use tokio::io::ReadBuf;
    use tokio_util::codec::Decoder;

    /// Transport of a client that sends nothing and accepts every write unless `blocked`.
    #[derive(Clone, Default)]
    struct MockIo(Arc<Mutex<MockState>>);

    #[derive(Default)]
    struct MockState {
        blocked: bool,
        written: Vec<u8>,
    }

    impl AsyncRead for MockIo {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for MockIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let mut state = self.0.lock().unwrap();
            if state.blocked {
                return Poll::Pending
            }
            state.written.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// Service of a connection that receives no requests.
    struct NoCalls;

    impl Service<String> for NoCalls {
        type Response = Option<String>;
        type Error = io::Error;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: String) -> Self::Future {
            std::future::ready(Ok(None))
        }
    }

    /// Records whether the driver woke itself.
    #[derive(Default)]
    struct Woken(AtomicBool);

    impl Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    /// Polls the driver until it is pending without having woken itself.
    fn poll_until_parked(mut driver: Pin<&mut impl Future<Output = ()>>) {
        let woken = Arc::new(Woken::default());
        let waker = Waker::from(woken.clone());
        let mut cx = Context::from_waker(&waker);
        loop {
            woken.0.store(false, Ordering::Relaxed);
            assert!(driver.as_mut().poll(&mut cx).is_pending());
            if !woken.0.load(Ordering::Relaxed) {
                return
            }
        }
    }

    #[test]
    fn takes_waiting_messages_after_draining_full_queue() {
        let io = MockIo::default();
        io.0.lock().unwrap().blocked = true;
        let (tx, rx) = mpsc::channel(MESSAGE_BUFFER_CAPACITY);
        let mut driver = pin!(IpcConnDriver {
            conn: IpcConn(StreamCodec::stream_incoming().framed(io.clone())),
            service: NoCalls,
            pending_calls: Default::default(),
            items: Default::default(),
            messages: rx,
        });

        // The client doesn't read, so the send queue fills up and messages wait in the channel.
        let mut sent = 0;
        loop {
            while tx.try_send(format!("<{sent}>")).is_ok() {
                sent += 1;
            }
            poll_until_parked(driver.as_mut());
            if tx.capacity() == 0 {
                break
            }
        }
        assert!(io.0.lock().unwrap().written.is_empty());

        // The client reads again, fast enough that the whole queue is written at once. The
        // messages that waited in the channel must follow without anything else waking the driver.
        io.0.lock().unwrap().blocked = false;
        poll_until_parked(driver.as_mut());

        let written = String::from_utf8(io.0.lock().unwrap().written.clone()).unwrap();
        assert_eq!(written.matches('<').count(), sent);
    }
}
