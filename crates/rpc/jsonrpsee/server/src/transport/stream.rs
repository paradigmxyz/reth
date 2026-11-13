//! Serves JSON-RPC over a connection that carries whole messages in both directions, such as a
//! WebSocket or an IPC socket.
//!
//! The transport supplies the incoming messages as a `Stream` and a `MessageWriter` for the
//! outgoing ones; `serve_stream` runs the calls and writes their responses.

use std::{fmt, future::Future, sync::Arc, time::Duration};

use crate::{
    server::{handle_rpc_call, BatchRequestConfig},
    LOG_TARGET,
};

use futures_util::{future, Sink, SinkExt, Stream, StreamExt};
use jsonrpsee_core::{classify_message, middleware::RpcServiceT, server::MethodSink};
use jsonrpsee_types::{
    error::{reject_too_big_request, ErrorCode},
    Id,
};
use tokio::{
    sync::{mpsc, oneshot, Semaphore},
    time::{interval_at, Instant, Interval},
};
use tokio_util::task::TaskTracker;

/// Most queued messages written before a flush, which bounds how long written messages wait in
/// the buffer.
const MAX_MESSAGES_PER_FLUSH: usize = 64;

/// Messages the receive loop handles before it lets the send loop write the queued responses.
/// Both loops share the connection task, so a client that keeps sending would otherwise delay
/// its own responses.
const MAX_MESSAGES_PER_TURN: usize = 32;

/// A connection to serve with [`serve_stream`].
#[derive(Debug)]
pub struct StreamConnection<R, W, S, F> {
    /// The messages received from the client. The stream ends when the client disconnects or the
    /// connection can't be read anymore.
    pub incoming: R,
    /// Writes the messages for the client.
    pub outgoing: W,
    /// Runs the calls.
    pub rpc_service: S,
    /// Queues messages for the client. Subscriptions of `rpc_service` send through a clone.
    pub sink: MethodSink,
    /// The messages queued through `sink`.
    pub rx: mpsc::Receiver<String>,
    /// Resolves when the server stops. No further message is read then, the running calls are
    /// answered and the connection is closed.
    pub stopped: F,
    /// How batches are handled.
    pub batch_requests_config: BatchRequestConfig,
    /// The limit named in the answer to a [`TooLarge`] message.
    pub max_request_body_size: u32,
    /// Calls the connection runs at once. Further messages are not read while that many run.
    pub max_concurrent_calls: usize,
    /// Interval at which the client is pinged with [`MessageWriter::ping`], if at all.
    pub ping_interval: Option<Duration>,
}

/// Serves `conn` until the client disconnects, a write fails or the server stops.
///
/// Every message runs as its own task and is answered through `conn.sink`, in the order the calls
/// complete. Messages are read and written by the calling task.
pub async fn serve_stream<R, M, W, S, F>(conn: StreamConnection<R, W, S, F>)
where
    R: Stream<Item = Result<M, TooLarge>> + Unpin,
    M: AsRef<[u8]> + Send + 'static,
    W: MessageWriter,
    S: RpcServiceT + Send + Sync + 'static,
    F: Future<Output = ()>,
{
    let StreamConnection {
        incoming,
        outgoing,
        rpc_service,
        sink,
        rx,
        stopped,
        batch_requests_config,
        max_request_body_size,
        max_concurrent_calls,
        ping_interval,
    } = conn;

    let (conn_tx, conn_rx) = oneshot::channel();
    // Tracks the running calls, so that a graceful shutdown can wait for them.
    let calls = TaskTracker::new();

    // The responses are written by this task too, so that a response doesn't have to wake another
    // task to reach the socket.
    let send = send_loop(rx, outgoing, ping_interval, conn_rx);

    let receive = async move {
        let rpc_service = Arc::new(rpc_service);
        let mut incoming = incoming.fuse();
        let call_permits = Arc::new(Semaphore::new(max_concurrent_calls));
        tokio::pin!(stopped);

        // Messages handled since this loop last yielded to the send loop.
        let mut handled = 0;

        let shutdown = loop {
            // Nothing is given up while no response is queued, so a quiet connection never yields
            // here.
            handled += 1;
            if handled >= MAX_MESSAGES_PER_TURN && sink.capacity() != sink.max_capacity() {
                handled = 0;
                tokio::task::yield_now().await;
            }

            // The send loop has ended, e.g. because a write timed out, so nothing can be sent back
            // to the client anymore. A client that keeps sending never lets the select below get
            // to `sink.closed()`, so it is checked here too.
            if sink.is_closed() {
                break Shutdown::ConnectionClosed;
            }
            let received = tokio::select! {
                // The stop signal is polled first: a peer that always has another message ready
                // never lets the read be pending, and the session would then never stop.
                biased;
                _ = &mut stopped => break Shutdown::Stopped,
                received = incoming.next() => received,
                _ = sink.closed() => break Shutdown::ConnectionClosed,
            };
            let data = match received {
                // The connection is closed.
                None => break Shutdown::ConnectionClosed,
                Some(Ok(data)) => data,
                Some(Err(TooLarge)) => {
                    if sink
                        .send_error(Id::Null, reject_too_big_request(max_request_body_size))
                        .await
                        .is_err()
                    {
                        break Shutdown::ConnectionClosed;
                    }

                    continue;
                }
            };

            // A connection runs at most `max_concurrent_calls` calls at once. While waiting for a
            // permit no further messages are read, which pushes back on the client, and the
            // message that is waiting is the only one held. The permit is acquired after the read
            // so that a connection at the limit that sends nothing more is still read and its
            // close is noticed. Every call ends once the sink is closed, which frees the permits,
            // so this wait ends when the connection closes.
            let permit = tokio::select! {
                permit = call_permits.clone().acquire_owned() => permit,
                _ = &mut stopped => break Shutdown::Stopped,
            };
            // The semaphore is never closed.
            let Ok(permit) = permit else { break Shutdown::ConnectionClosed };

            let rpc_service = rpc_service.clone();
            let sink = sink.clone();

            calls.spawn(async move {
                let _permit = permit;
                let Some((data, is_single)) = classify_message(data.as_ref()) else {
                    _ = sink.send_error(Id::Null, ErrorCode::ParseError.into()).await;
                    return;
                };

                // Calls are not tied to the connection task, so stop them once the connection is
                // closed instead of running them to completion without anyone to send the
                // response to.
                let rp = tokio::select! {
                    // The call is polled first, so one that answers at once doesn't have to
                    // register for the close of the connection.
                    biased;
                    rp = handle_rpc_call(data, is_single, batch_requests_config, &*rpc_service) => rp,
                    _ = sink.closed() => return,
                };

                // Subscriptions are handled by the subscription callback and
                // "ordinary notifications" should not be sent back to the client.
                if rp.is_method_call() || rp.is_batch() {
                    // An error means the connection is closed, so there is nothing left to do.
                    let _ = sink.send(rp.into_json()).await;
                }
            });
        };

        // Drive all running methods to completion.
        // **NOTE** Do not return early in this block. This `await` needs to run to guarantee
        // proper drop behaviour.
        drop(rpc_service);
        calls.close();
        graceful_shutdown(shutdown, &calls, &mut incoming, conn_tx).await;

        // Returned so that the connection stays open for reading until the send loop has ended.
        incoming
    };

    // Both loops share the task's budget, so neither may always go first: a client that keeps
    // sending would use it all in the receive loop, and a subscription that keeps the queue full
    // would use it all in the send loop. `tokio::join!` alternates which one it polls first.
    let ((), _incoming) = tokio::join!(send, receive);
}

/// A received message exceeds the maximum request size. It is answered with an error and the
/// connection continues.
#[derive(Debug, Clone, Copy)]
pub struct TooLarge;

/// Writes messages to a connection.
///
/// Implemented for every [`Sink`] of `String`s.
pub trait MessageWriter: Send {
    /// Error of a failed write.
    type Error: fmt::Display + Send;

    /// Writes a message, which may stay buffered until [`Self::flush`].
    fn write(&mut self, message: String) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Writes a ping, which may stay buffered until [`Self::flush`]. Does nothing by default, for
    /// connections that are not pinged.
    fn ping(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { Ok(()) }
    }

    /// Sends the buffered messages.
    fn flush(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Closes the connection for writing. Also called after a failed write, so a writer whose
    /// write timed out must not block here.
    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

impl<T> MessageWriter for T
where
    T: Sink<String> + Unpin + Send,
    T::Error: fmt::Display + Send,
{
    type Error = T::Error;

    fn write(&mut self, message: String) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.feed(message)
    }

    fn flush(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        SinkExt::flush(self)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        SinkExt::close(self)
    }
}

/// Waits for new messages via the `rx` channel and writes them to the connection, and pings the
/// client every `ping_interval`.
async fn send_loop<W: MessageWriter>(
    mut rx: mpsc::Receiver<String>,
    mut outgoing: W,
    ping_interval: Option<Duration>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut ping_timer =
        ping_interval.map(|interval| interval_at(Instant::now() + interval, interval));

    loop {
        let mut result = tokio::select! {
            // Stop is polled first: a subscription that queues messages faster than the client
            // reads them keeps `rx.recv()` ready, which would never let a stopped server end the
            // session.
            biased;
            _ = &mut stop, if !rx.is_closed() => {
                // Take nothing new but write out what is queued: answers to calls this server has
                // run, which a graceful shutdown owes its clients. `rx.recv()` returns `None` once
                // the closed queue is drained.
                rx.close();
                continue;
            }
            // Pings go before queued messages, so that a subscription that keeps the queue full
            // can't hold them back until the read timeout closes the connection.
            _ = ping_due(&mut ping_timer) => outgoing.ping().await,
            response = rx.recv() => {
                // Nothing else to receive.
                let Some(response) = response else { break };
                outgoing.write(response).await
            }
        };
        // Write the messages that are already queued too, and flush them together.
        let mut written = 1;
        while result.is_ok() && written < MAX_MESSAGES_PER_FLUSH {
            let Ok(response) = rx.try_recv() else { break };
            result = outgoing.write(response).await;
            written += 1;
        }
        if result.is_ok() {
            result = outgoing.flush().await;
        }

        // If a message can't be sent then terminate the connection.
        if let Err(err) = result {
            tracing::debug!(target: LOG_TARGET, "Send error: {}", err);
            break;
        }
    }

    // Closing `rx` makes `MethodSink::closed` resolve, which stops the receive loop, pending calls
    // and subscriptions of this connection. Messages still queued are dropped with it.
    rx.close();

    // The receive half is dropped by the receive loop, which closes the socket.
    let _ = outgoing.close().await;
}

/// Waits until the next ping is due, forever if the connection is not pinged.
async fn ping_due(ping_timer: &mut Option<Interval>) {
    match ping_timer {
        Some(ping_timer) => {
            ping_timer.tick().await;
        }
        None => future::pending().await,
    }
}

#[derive(Debug, Copy, Clone)]
enum Shutdown {
    Stopped,
    ConnectionClosed,
}

/// Enforce a graceful shutdown.
///
/// This will return once the connection has been terminated or all pending calls have been
/// executed.
async fn graceful_shutdown<R, M>(
    shutdown: Shutdown,
    calls: &TaskTracker,
    incoming: &mut R,
    mut conn_tx: oneshot::Sender<()>,
) where
    R: Stream<Item = Result<M, TooLarge>> + Unpin,
{
    if matches!(shutdown, Shutdown::Stopped) {
        // The wait also ends when the client disconnects, the connection can't be read anymore,
        // or a message is too large.
        let disconnect = async { while let Some(Ok(_)) = incoming.next().await {} };

        tokio::select! {
            _ = calls.wait() => {}
            _ = disconnect => {}
            _ = conn_tx.closed() => {}
        }
    }

    // Tell the send loop to write out what is queued and end.
    _ = conn_tx.send(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::rpc::RpcService;
    use futures_util::stream;
    use jsonrpsee_core::server::Methods;
    use std::{
        io,
        pin::Pin,
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// Counts the messages written to it, or fails every write.
    struct TestWriter {
        written: Arc<AtomicUsize>,
        fail: bool,
        /// Whether a write uses up the task's budget, as a write to a socket does.
        cooperative: bool,
    }

    impl MessageWriter for TestWriter {
        type Error = io::Error;

        async fn write(&mut self, _: String) -> Result<(), Self::Error> {
            if self.fail {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if self.cooperative {
                tokio::task::consume_budget().await;
            }
            self.written.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    type Incoming = Pin<Box<dyn Stream<Item = Result<Vec<u8>, TooLarge>> + Send>>;

    /// A connection of a client that always has another message ready. Like a socket, the stream
    /// gives way once the task has used up its budget.
    fn flooded<F>(
        outgoing: TestWriter,
        stopped: F,
    ) -> StreamConnection<Incoming, TestWriter, RpcService, F> {
        let always_ready = stream::unfold((), |()| async {
            tokio::task::consume_budget().await;
            Some((Ok(b"{}".to_vec()), ()))
        });
        let (tx, rx) = mpsc::channel(4);
        StreamConnection {
            incoming: Box::pin(always_ready),
            outgoing,
            rpc_service: RpcService::without_subscriptions(Methods::new(), usize::MAX, 0.into()),
            sink: MethodSink::new(tx),
            rx,
            stopped,
            batch_requests_config: BatchRequestConfig::Unlimited,
            max_request_body_size: u32::MAX,
            max_concurrent_calls: Semaphore::MAX_PERMITS,
            ping_interval: None,
        }
    }

    #[tokio::test]
    async fn no_message_is_read_once_stopped() {
        // Repeated, because a message read ahead of the stop signal would still be dropped half
        // of the time by the wait for a call permit, which observes the stop signal too.
        for _ in 0..32 {
            let written = Arc::new(AtomicUsize::new(0));
            let outgoing = TestWriter { written: written.clone(), fail: false, cooperative: false };

            let served = serve_stream(flooded(outgoing, future::ready(())));
            tokio::time::timeout(Duration::from_secs(5), served).await.unwrap();

            // Every message that is read is answered, with an error since `{}` is no request.
            assert_eq!(written.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn send_loop_runs_while_input_stays_ready() {
        // The first response fails to be written, which ends the send loop and with it the
        // connection. That only happens if the send loop gets to run.
        let outgoing = TestWriter { written: Arc::default(), fail: true, cooperative: false };

        let served = serve_stream(flooded(outgoing, future::pending()));
        tokio::time::timeout(Duration::from_secs(5), served).await.unwrap();
    }

    #[tokio::test]
    async fn receive_loop_runs_while_output_stays_ready() {
        // Counts the messages read from the client.
        let read = Arc::new(AtomicUsize::new(0));
        let counter = read.clone();
        let mut conn = flooded(
            TestWriter { written: Arc::default(), fail: false, cooperative: true },
            future::pending(),
        );
        conn.incoming = Box::pin(conn.incoming.inspect(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        }));

        // A subscription that refills the queue whenever the connection task gives way, so the
        // send loop always has another message to write.
        let (tx, rx) = mpsc::channel(1024);
        conn.sink = MethodSink::new(tx.clone());
        conn.rx = rx;
        // The queue is full from the start, so that the send loop has work before the first read.
        while tx.try_send(String::new()).is_ok() {}
        let producer = tokio::spawn(async move { while tx.send(String::new()).await.is_ok() {} });

        // The connection never ends by itself.
        let served = tokio::time::timeout(Duration::from_millis(200), serve_stream(conn)).await;
        assert!(served.is_err());
        producer.abort();

        assert!(read.load(Ordering::Relaxed) > 0);
    }
}
