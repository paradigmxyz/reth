use std::{io, sync::Arc, time::Duration};

use crate::{
    server::{handle_rpc_call, ConnectionState, ServerConfig},
    LOG_TARGET,
};

use futures_util::{
    future::{self, Either},
    stream::{SplitSink, SplitStream},
    Future, SinkExt, StreamExt, TryStreamExt,
};
use http::header::{CONNECTION, UPGRADE};
use hyper::{body::Bytes, upgrade::Upgraded};
use hyper_util::rt::TokioIo;
use jsonrpsee_core::{classify_message, middleware::RpcServiceT, server::MethodSink};
use jsonrpsee_types::{
    error::{reject_too_big_request, ErrorCode},
    Id,
};
use tokio::{
    sync::{mpsc, oneshot, Semaphore},
    time::{interval_at, timeout, timeout_at, Instant},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_tungstenite::{
    tungstenite::{
        self,
        error::CapacityError,
        protocol::{
            frame::{
                coding::{CloseCode, Data, OpCode},
                Frame,
            },
            CloseFrame,
        },
        Message, Utf8Bytes,
    },
    WebSocketStream,
};

pub(crate) type Sender = SplitSink<WebSocketStream<TokioIo<Upgraded>>, Message>;
pub(crate) type Receiver = SplitStream<WebSocketStream<TokioIo<Upgraded>>>;

/// Returns whether the request asks for an upgrade to the WebSocket protocol.
pub fn is_upgrade_request<B>(request: &http::Request<B>) -> bool {
    let has_token = |name, token: &[u8]| {
        request.headers().get_all(name).iter().any(|value| {
            value
                .as_bytes()
                .split(|&b| b == b',')
                .any(|listed| listed.trim_ascii().eq_ignore_ascii_case(token))
        })
    };
    has_token(CONNECTION, b"upgrade") && has_token(UPGRADE, b"websocket")
}

/// Most queued messages written before a flush, which bounds how long written messages wait in
/// the buffer.
const MAX_MESSAGES_PER_FLUSH: usize = 64;

/// Largest payload of a frame sent to the client; a longer message is sent in fragments.
/// `tungstenite` copies a frame into its write buffer whole, moves what a partial write left of it
/// to the front each time, and keeps the buffer's capacity for the connection.
const MAX_FRAME_PAYLOAD: usize = 64 * 1024;

pub(crate) struct BackgroundTaskParams<S> {
    pub(crate) server_cfg: ServerConfig,
    pub(crate) conn: ConnectionState,
    pub(crate) ws_sender: Sender,
    pub(crate) ws_receiver: Receiver,
    pub(crate) rpc_service: S,
    pub(crate) sink: MethodSink,
    pub(crate) rx: mpsc::Receiver<String>,
    /// Held by every running call, so that a graceful shutdown can wait for them.
    pub(crate) pending_calls: mpsc::Sender<()>,
    pub(crate) pending_calls_completed: mpsc::Receiver<()>,
}

pub(crate) async fn background_task<S>(params: BackgroundTaskParams<S>)
where
    S: RpcServiceT + Send + Sync + 'static,
{
    let BackgroundTaskParams {
        server_cfg,
        conn,
        ws_sender,
        ws_receiver,
        rpc_service,
        sink,
        rx,
        pending_calls,
        pending_calls_completed,
    } = params;
    let ServerConfig {
        batch_requests_config,
        max_request_body_size,
        write_timeout,
        message_buffer_capacity,
        ping_interval,
        ..
    } = server_cfg;

    let (conn_tx, conn_rx) = oneshot::channel();
    // The receive stream asks the send task to write out the pongs. It holds one request, so the
    // receive stream waits while a write is blocked.
    let (pong_tx, pong_rx) = mpsc::channel(1);

    // Spawn another task that sends out the responses on the Websocket.
    let send_task_handle =
        tokio::spawn(send_task(rx, ws_sender, write_timeout, ping_interval, pong_rx, conn_rx));

    let stopped = conn.stop_handle.clone().shutdown();
    let rpc_service = Arc::new(rpc_service);

    tokio::pin!(stopped);

    // `tungstenite` reserves a frame's announced length before its payload arrives, so a frame that
    // never completes would hold that buffer for good. A pong can't arrive inside a frame, so a
    // read that yields neither a message nor a pong to the pings within two intervals is stuck.
    let read_timeout = ping_interval * 2;
    let ws_stream = futures_util::stream::unfold(Some(ws_receiver), move |receiver| {
        let pong_tx = pong_tx.clone();
        async move {
            let mut receiver = receiver?;
            let mut deadline = Instant::now() + read_timeout;
            loop {
                let Ok(received) = timeout_at(deadline, receiver.next()).await else {
                    return Some((
                        Err(tungstenite::Error::Io(io::ErrorKind::TimedOut.into())),
                        Some(receiver),
                    ));
                };
                match received? {
                    Ok(Message::Text(text)) => return Some((Ok(text.into()), Some(receiver))),
                    Ok(Message::Binary(data)) => return Some((Ok(data), Some(receiver))),
                    // Pongs only answer the server's pings, which restarts the read timeout.
                    Ok(Message::Pong(_)) => deadline = Instant::now() + read_timeout,
                    // `tungstenite` has queued the pong and would write it with a later read or
                    // write. The send task flushes it instead, so that pongs can't pile up in the
                    // write buffer of a client that isn't reading.
                    Ok(Message::Ping(_)) => {
                        // The send task has ended, and the connection with it.
                        if pong_tx.send(()).await.is_err() {
                            return None;
                        }
                    }
                    Ok(Message::Close(_)) => return None,
                    // Raw frames are only written, never read.
                    Ok(Message::Frame(_)) => {}
                    // `tungstenite` reads nothing more after an error, so the stream ends with it.
                    Err(e) => return Some((Err(e), None)),
                }
            }
        }
    })
    .fuse();

    tokio::pin!(ws_stream);

    let call_permits = Arc::new(Semaphore::new(message_buffer_capacity as usize));

    let result = loop {
        let receive = tokio::select! {
            receive = try_recv(&mut ws_stream, stopped) => receive,
            // The send task has terminated, e.g. because a write timed out, so nothing can be sent
            // back to the client anymore.
            _ = sink.closed() => break Ok(Shutdown::ConnectionClosed),
        };
        let data = match receive {
            Receive::ConnectionClosed => break Ok(Shutdown::ConnectionClosed),
            Receive::Stopped => break Ok(Shutdown::Stopped),
            Receive::Ok(data, stop) => {
                stopped = stop;
                data
            }
            Receive::Err(err, stop) => {
                stopped = stop;

                match err {
                    tungstenite::Error::Capacity(CapacityError::MessageTooLong {
                        size: current,
                        max_size: maximum,
                    }) => {
                        tracing::debug!(
                            target: LOG_TARGET,
                            "WS recv error: message too large current={}/max={}",
                            current,
                            maximum
                        );
                        if sink
                            .send_error(Id::Null, reject_too_big_request(max_request_body_size))
                            .await
                            .is_err()
                        {
                            break Ok(Shutdown::ConnectionClosed);
                        }

                        // The stream has ended, so the next read closes the connection after the
                        // rejection is written.
                        continue;
                    }
                    err => {
                        tracing::debug!(target: LOG_TARGET, "WS error: {}; terminate connection: {}", err, conn.conn_id);
                        break Err(err);
                    }
                };
            }
        };

        // A connection runs at most `message_buffer_capacity` calls at once. While waiting for a
        // permit no further messages are read, which pushes back on the client, and the message
        // that is waiting is the only one held. The permit is acquired after the read so that a
        // connection at the limit that sends nothing more is still read and its close is noticed.
        // Every call ends once the sink is closed, which frees the permits, so this wait ends
        // when the connection closes.
        let permit = tokio::select! {
            permit = call_permits.clone().acquire_owned() => permit,
            _ = &mut stopped => break Ok(Shutdown::Stopped),
        };
        // The semaphore is never closed.
        let Ok(permit) = permit else { break Ok(Shutdown::ConnectionClosed) };

        let rpc_service = rpc_service.clone();
        let sink = sink.clone();
        let pending_calls = pending_calls.clone();

        tokio::spawn(async move {
            let _pending_calls = pending_calls;
            let _permit = permit;
            let Some((data, is_single)) = classify_message(&data) else {
                _ = sink.send_error(Id::Null, ErrorCode::ParseError.into()).await;
                return;
            };

            // Calls are not tied to the connection task, so stop them once the connection is closed
            // instead of running them to completion without anyone to send the response to.
            let rp = tokio::select! {
                // The call is polled first, so one that answers at once doesn't have to register
                // for the close of the connection.
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
    // **NOTE** Do not return early in this function. This `await` needs to run to guarantee
    // proper drop behaviour.
    drop(rpc_service);
    drop(pending_calls);
    graceful_shutdown(result, pending_calls_completed, ws_stream, conn_tx, send_task_handle).await;

    drop(conn);
}

/// A task that waits for new messages via the `rx channel` and sends them out on the `WebSocket`,
/// and pings the client every `ping_interval`.
async fn send_task(
    mut rx: mpsc::Receiver<String>,
    mut ws_sender: Sender,
    write_timeout: Duration,
    ping_interval: Duration,
    mut pong_rx: mpsc::Receiver<()>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut write_timed_out = false;
    let mut ping_timer = interval_at(Instant::now() + ping_interval, ping_interval);

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
            _ = ping_timer.tick() => {
                timeout(write_timeout, ws_sender.feed(Message::Ping(Bytes::new()))).await
            }
            // Pongs go before queued messages too, as the receive stream waits for them. The
            // flush below writes the pong.
            Some(()) = pong_rx.recv() => Ok(Ok(())),
            response = rx.recv() => {
                // Nothing else to receive.
                let Some(response) = response else { break };
                timeout(write_timeout, feed_text(&mut ws_sender, response)).await
            }
        };
        // Write the messages that are already queued too, and flush them together.
        let mut written = 1;
        while matches!(result, Ok(Ok(()))) && written < MAX_MESSAGES_PER_FLUSH {
            let Ok(response) = rx.try_recv() else { break };
            result = timeout(write_timeout, feed_text(&mut ws_sender, response)).await;
            written += 1;
        }
        if matches!(result, Ok(Ok(()))) {
            result = timeout(write_timeout, ws_sender.flush()).await;
        }

        // If websocket message send fail then terminate the connection.
        match result {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::debug!(target: LOG_TARGET, "WS send error: {}", err);
                break;
            }
            Err(_) => {
                tracing::debug!(target: LOG_TARGET, "WS send timed out after {write_timeout:?}");
                write_timed_out = true;
                break;
            }
        }
    }

    // Closing `rx` makes `MethodSink::closed` resolve, which stops the receive loop, pending calls
    // and subscriptions of this connection. Messages still queued are dropped with this task.
    rx.close();

    // Send the close message, unless a write already timed out: the client isn't reading, so the
    // close frame would block on the same full socket. The receive half is dropped by the
    // connection task, which closes the socket.
    if !write_timed_out {
        let close = Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: Utf8Bytes::default(),
        }));
        let _ = timeout(write_timeout, async {
            // Refused after a close frame from the client, whose reply `tungstenite` has queued
            // and `close` then writes. The split sink only hands a fed message on with the next
            // flush or close, so it is sent here: a refusal in `close` would skip writing the
            // reply.
            let _ = ws_sender.send(close).await;
            ws_sender.close().await
        })
        .await;
    }
}

/// Queues `text` as one message, in frames of at most [`MAX_FRAME_PAYLOAD`] bytes.
async fn feed_text(ws_sender: &mut Sender, text: String) -> Result<(), tungstenite::Error> {
    let mut payload = Bytes::from(text);
    let mut opcode = OpCode::Data(Data::Text);
    loop {
        let fragment = payload.split_to(payload.len().min(MAX_FRAME_PAYLOAD));
        let frame = Frame::message(fragment, opcode, payload.is_empty());
        ws_sender.feed(Message::Frame(frame)).await?;
        if payload.is_empty() {
            return Ok(());
        }
        opcode = OpCode::Data(Data::Continue);
    }
}

enum Receive<S> {
    ConnectionClosed,
    Stopped,
    Err(tungstenite::Error, S),
    Ok(Bytes, S),
}

/// Attempts to read data from WebSocket fails if the server was stopped.
async fn try_recv<T, S>(ws_stream: &mut T, stopped: S) -> Receive<S>
where
    S: Future<Output = ()> + Unpin,
    T: StreamExt<Item = Result<Bytes, tungstenite::Error>> + Unpin,
{
    // `select` polls its first argument first and returns as soon as that one is ready, so the stop
    // signal has to come first. Polling the stream first starves it: a peer that always has another
    // message ready never lets the read be pending, and the session would then never stop.
    match future::select(stopped, ws_stream.next()).await {
        // Server has been stopped.
        Either::Left(_) => Receive::Stopped,
        // The connection is closed.
        Either::Right((None, _)) => Receive::ConnectionClosed,
        // The message has been received, we are done
        Either::Right((Some(Ok(data)), stopped)) => Receive::Ok(data, stopped),
        // Received an error, terminate the connection.
        Either::Right((Some(Err(e)), stopped)) => Receive::Err(e, stopped),
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum Shutdown {
    Stopped,
    ConnectionClosed,
}

/// Enforce a graceful shutdown.
///
/// This will return once the connection has been terminated or all pending calls have been
/// executed.
async fn graceful_shutdown<S>(
    result: Result<Shutdown, tungstenite::Error>,
    pending_calls: mpsc::Receiver<()>,
    ws_stream: S,
    mut conn_tx: oneshot::Sender<()>,
    send_task_handle: tokio::task::JoinHandle<()>,
) where
    S: StreamExt<Item = Result<Bytes, tungstenite::Error>> + Unpin,
{
    let pending_calls = ReceiverStream::new(pending_calls);

    if let Ok(Shutdown::Stopped) = result {
        let graceful_shutdown = pending_calls.for_each(|_| async {});
        let disconnect = ws_stream.try_for_each(|_| async { Ok(()) });

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

    // Send a message to close down the "send task".
    _ = conn_tx.send(());
    // Ensure that send task has been closed.
    _ = send_task_handle.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    #[tokio::test]
    async fn try_recv_observes_stop_while_input_stays_ready() {
        // A peer that always has another message ready must not keep the session from stopping.
        let mut always_ready =
            stream::repeat_with(|| Ok::<_, tungstenite::Error>(Bytes::from_static(b"{}")));

        let received = try_recv(&mut always_ready, future::ready(())).await;
        assert!(matches!(received, Receive::Stopped));
    }

    #[test]
    fn upgrade_request_is_detected_in_header_lists() {
        let request = |connection, upgrade| {
            http::Request::builder()
                .header(CONNECTION, connection)
                .header(UPGRADE, upgrade)
                .body(())
                .unwrap()
        };

        assert!(is_upgrade_request(&request("Upgrade", "websocket")));
        assert!(is_upgrade_request(&request("keep-alive, Upgrade", "h2c, WebSocket")));
        assert!(!is_upgrade_request(&request("keep-alive", "websocket")));
        assert!(!is_upgrade_request(&request("upgrade", "h2c")));
        assert!(!is_upgrade_request(&http::Request::new(())));
    }
}
