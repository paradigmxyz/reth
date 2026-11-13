//! Serves JSON-RPC over a connection that carries whole messages in both directions, such as a
//! WebSocket or an IPC socket.
//!
//! The transport supplies the incoming messages as a [`Stream`] and a [`MessageWriter`] for the
//! outgoing ones; [`serve_stream`] runs the calls and writes their responses.

use std::{fmt, future::Future, sync::Arc, time::Duration};

use crate::{
    server::{handle_rpc_call, BatchRequestConfig},
    LOG_TARGET,
};

use futures_util::{
    future::{self, Either},
    Sink, SinkExt, Stream, StreamExt,
};
use jsonrpsee_core::{classify_message, middleware::RpcServiceT, server::MethodSink};
use jsonrpsee_types::{
    error::{reject_too_big_request, ErrorCode},
    Id,
};
use tokio::{
    sync::{mpsc, oneshot, Semaphore},
    time::{interval_at, Instant, Interval},
};

/// Most queued messages written before a flush, which bounds how long written messages wait in
/// the buffer.
const MAX_MESSAGES_PER_FLUSH: usize = 64;

/// Why a message could not be read from a connection.
#[derive(Debug)]
pub enum ReadError<E> {
    /// The message exceeds the maximum request size. It is answered with an error and the
    /// connection continues.
    TooLarge,
    /// The connection can't be read anymore and is closed.
    Fatal(E),
}

impl<E: fmt::Display> fmt::Display for ReadError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("message too large"),
            Self::Fatal(err) => err.fmt(f),
        }
    }
}

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

    /// Closes the connection for writing.
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

/// A connection to serve with [`serve_stream`].
#[derive(Debug)]
pub struct StreamConnection<R, W, S, F> {
    /// The messages received from the client. The stream ends when the client disconnects.
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
    /// The limit named in the answer to a [`ReadError::TooLarge`] message.
    pub max_request_body_size: u32,
    /// Calls the connection runs at once. Further messages are not read while that many run.
    pub max_concurrent_calls: usize,
    /// How long writing a single message, or a flush, may take before the connection is closed.
    pub write_timeout: Option<Duration>,
    /// Interval at which the client is pinged with [`MessageWriter::ping`], if at all.
    pub ping_interval: Option<Duration>,
}

/// Serves `conn` until the client disconnects, a write fails or the server stops.
///
/// Every message runs as its own task and is answered through `conn.sink`, in the order the calls
/// complete. Messages are read and written by the calling task.
pub async fn serve_stream<R, M, E, W, S, F>(conn: StreamConnection<R, W, S, F>)
where
    R: Stream<Item = Result<M, ReadError<E>>> + Unpin,
    M: AsRef<[u8]> + Send + 'static,
    E: fmt::Display,
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
        write_timeout,
        ping_interval,
    } = conn;

    let (conn_tx, conn_rx) = oneshot::channel();
    // Held by every running call, so that a graceful shutdown can wait for them.
    let (pending_calls, pending_calls_completed) = mpsc::channel::<()>(1);

    // The responses are written by this task too, so that a response doesn't have to wake another
    // task to reach the socket.
    let send = send_loop(rx, outgoing, write_timeout, ping_interval, conn_rx);

    let receive = async move {
        let rpc_service = Arc::new(rpc_service);
        let mut incoming = incoming.fuse();
        let call_permits = Arc::new(Semaphore::new(max_concurrent_calls));
        tokio::pin!(stopped);

        let shutdown = loop {
            let receive = tokio::select! {
                receive = try_recv(&mut incoming, stopped) => receive,
                // The send loop has ended, e.g. because a write timed out, so nothing can be sent
                // back to the client anymore.
                _ = sink.closed() => break Shutdown::ConnectionClosed,
            };
            let data = match receive {
                Receive::ConnectionClosed => break Shutdown::ConnectionClosed,
                Receive::Stopped => break Shutdown::Stopped,
                Receive::Ok(data, stop) => {
                    stopped = stop;
                    data
                }
                Receive::Err(ReadError::TooLarge, stop) => {
                    stopped = stop;

                    if sink
                        .send_error(Id::Null, reject_too_big_request(max_request_body_size))
                        .await
                        .is_err()
                    {
                        break Shutdown::ConnectionClosed;
                    }

                    continue;
                }
                // The transport has logged the error.
                Receive::Err(ReadError::Fatal(_), _) => break Shutdown::ConnectionClosed,
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
            let pending_calls = pending_calls.clone();

            tokio::spawn(async move {
                let _pending_calls = pending_calls;
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
        drop(pending_calls);
        graceful_shutdown(shutdown, pending_calls_completed, &mut incoming, conn_tx).await;

        // Returned so that the connection stays open for reading until the send loop has ended.
        incoming
    };

    let (_incoming, ()) = future::join(receive, send).await;
}

/// Waits for new messages via the `rx` channel and writes them to the connection, and pings the
/// client every `ping_interval`.
async fn send_loop<W: MessageWriter>(
    mut rx: mpsc::Receiver<String>,
    mut outgoing: W,
    write_timeout: Option<Duration>,
    ping_interval: Option<Duration>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut write_timed_out = false;
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
            _ = ping_due(&mut ping_timer) => within(write_timeout, outgoing.ping()).await,
            response = rx.recv() => {
                // Nothing else to receive.
                let Some(response) = response else { break };
                within(write_timeout, outgoing.write(response)).await
            }
        };
        // Write the messages that are already queued too, and flush them together.
        let mut written = 1;
        while matches!(result, Ok(Ok(()))) && written < MAX_MESSAGES_PER_FLUSH {
            let Ok(response) = rx.try_recv() else { break };
            result = within(write_timeout, outgoing.write(response)).await;
            written += 1;
        }
        if matches!(result, Ok(Ok(()))) {
            result = within(write_timeout, outgoing.flush()).await;
        }

        // If a message can't be sent then terminate the connection.
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::debug!(target: LOG_TARGET, "Send error: {}", err);
                break;
            }
            Err(WriteTimedOut) => {
                tracing::debug!(target: LOG_TARGET, "Send timed out after {write_timeout:?}");
                write_timed_out = true;
                break;
            }
        }
    }

    // Closing `rx` makes `MethodSink::closed` resolve, which stops the receive loop, pending calls
    // and subscriptions of this connection. Messages still queued are dropped with it.
    rx.close();

    // Close the connection, unless a write already timed out: the client isn't reading, so the
    // close would block on the same full socket. The receive half is dropped by the receive loop,
    // which closes the socket.
    if !write_timed_out {
        let _ = within(write_timeout, outgoing.close()).await;
    }
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

/// A write took longer than the write timeout.
struct WriteTimedOut;

/// Runs `fut`, for at most `limit` if there is one.
async fn within<T>(
    limit: Option<Duration>,
    fut: impl Future<Output = T>,
) -> Result<T, WriteTimedOut> {
    match limit {
        Some(limit) => tokio::time::timeout(limit, fut).await.map_err(|_| WriteTimedOut),
        None => Ok(fut.await),
    }
}

enum Receive<M, E, S> {
    ConnectionClosed,
    Stopped,
    Err(ReadError<E>, S),
    Ok(M, S),
}

/// Attempts to read a message from the connection, unless the server was stopped.
async fn try_recv<R, M, E, S>(incoming: &mut R, stopped: S) -> Receive<M, E, S>
where
    R: Stream<Item = Result<M, ReadError<E>>> + Unpin,
    S: Future<Output = ()> + Unpin,
{
    // `select` polls its first argument first and returns as soon as that one is ready, so the stop
    // signal has to come first. Polling the stream first starves it: a peer that always has another
    // message ready never lets the read be pending, and the session would then never stop.
    match future::select(stopped, incoming.next()).await {
        // Server has been stopped.
        Either::Left(_) => Receive::Stopped,
        // The connection is closed.
        Either::Right((None, _)) => Receive::ConnectionClosed,
        // The message has been received, we are done
        Either::Right((Some(Ok(data)), stopped)) => Receive::Ok(data, stopped),
        // Received an error.
        Either::Right((Some(Err(e)), stopped)) => Receive::Err(e, stopped),
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
async fn graceful_shutdown<R, M, E>(
    shutdown: Shutdown,
    mut pending_calls: mpsc::Receiver<()>,
    incoming: &mut R,
    mut conn_tx: oneshot::Sender<()>,
) where
    R: Stream<Item = Result<M, ReadError<E>>> + Unpin,
    E: fmt::Display,
{
    if matches!(shutdown, Shutdown::Stopped) {
        let graceful_shutdown = async { while pending_calls.recv().await.is_some() {} };
        let disconnect = async {
            while let Some(received) = incoming.next().await {
                received?;
            }
            Ok::<_, ReadError<E>>(())
        };

        tokio::select! {
            _ = graceful_shutdown => {}
            res = disconnect => {
                if let Err(err) = res {
                    tracing::warn!(target: LOG_TARGET, "Graceful shutdown terminated because of error: `{err}`");
                }
            }
            _ = conn_tx.closed() => {}
        }
    }

    // Tell the send loop to write out what is queued and end.
    _ = conn_tx.send(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[tokio::test]
    async fn try_recv_observes_stop_while_input_stays_ready() {
        // A peer that always has another message ready must not keep the session from stopping.
        let mut always_ready =
            stream::repeat_with(|| Ok::<_, ReadError<std::io::Error>>(b"{}".to_vec()));

        let received = try_recv(&mut always_ready, future::ready(())).await;
        assert!(matches!(received, Receive::Stopped));
    }
}
