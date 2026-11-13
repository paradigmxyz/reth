use std::{sync::Arc, time::Duration};

use crate::{
    server::{handle_rpc_call, ConnectionState, ServerConfig},
    LOG_TARGET,
};

use futures_util::{
    future::{self, Either},
    io::{BufReader, BufWriter},
    Future, StreamExt, TryStreamExt,
};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use jsonrpsee_core::{classify_message, middleware::RpcServiceT, server::MethodSink};
use jsonrpsee_types::{
    error::{reject_too_big_request, ErrorCode},
    Id,
};
use soketto::connection::Error as SokettoError;
use tokio::{
    sync::{mpsc, oneshot, Semaphore},
    time::timeout,
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::Compat;

pub(crate) type Sender = soketto::Sender<BufReader<BufWriter<Compat<TokioIo<Upgraded>>>>>;
pub(crate) type Receiver = soketto::Receiver<BufReader<BufWriter<Compat<TokioIo<Upgraded>>>>>;

pub use soketto::handshake::http::is_upgrade_request;

/// Most queued messages written before a flush, which bounds how long written messages wait in
/// the buffer.
const MAX_MESSAGES_PER_FLUSH: usize = 64;

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
        ..
    } = server_cfg;

    let (conn_tx, conn_rx) = oneshot::channel();

    // The responses are sent out by this task too, so that a response doesn't have to wake another
    // task to reach the socket.
    let send = send_loop(rx, ws_sender, write_timeout, conn_rx);

    let stopped = conn.stop_handle.clone().shutdown();
    let rpc_service = Arc::new(rpc_service);

    tokio::pin!(stopped);

    let ws_stream = futures_util::stream::unfold(ws_receiver, |mut receiver| async {
        let mut data = Vec::new();
        loop {
            match receiver.receive(&mut data).await {
                Ok(soketto::Incoming::Data(_)) => return Some((Ok(data), receiver)),
                // The server sends no pings, so pongs are unsolicited and ignored.
                Ok(soketto::Incoming::Pong(_)) => data.clear(),
                Ok(soketto::Incoming::Closed(_)) | Err(SokettoError::Closed) => return None,
                // The closing reason is already logged by `soketto` trace log level.
                // Return the `Closed` error to avoid logging unnecessary warnings on clean
                // shutdown.
                Err(e) => return Some((Err(e), receiver)),
            }
        }
    })
    .fuse();

    tokio::pin!(ws_stream);

    let receive = async {
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
                        SokettoError::Closed => {
                            break Ok(Shutdown::ConnectionClosed);
                        }
                        SokettoError::MessageTooLarge { current, maximum } => {
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

                            continue;
                        }
                        err => {
                            tracing::debug!(target: LOG_TARGET, "WS error: {}; terminate connection: {}", err, conn.conn_id);
                            break Err(err);
                        }
                    };
                }
            };

            // A connection runs at most `message_buffer_capacity` calls at once. While waiting for
            // a permit no further messages are read, which pushes back on the client, and the
            // message that is waiting is the only one held. The permit is acquired after the read
            // so that a connection at the limit that sends nothing more is still read and its close
            // is noticed. Every call ends once the sink is closed, which frees the permits, so this
            // wait ends when the connection closes.
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

                // Calls are not tied to the connection task, so stop them once the connection is
                // closed instead of running them to completion without anyone to send the response
                // to.
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
        graceful_shutdown(result, pending_calls_completed, ws_stream.as_mut(), conn_tx).await;
    };

    future::join(receive, send).await;

    drop(conn);
}

/// Waits for new messages via the `rx channel` and sends them out on the `WebSocket`.
async fn send_loop(
    mut rx: mpsc::Receiver<String>,
    mut ws_sender: Sender,
    write_timeout: Duration,
    mut stop: oneshot::Receiver<()>,
) {
    let mut write_timed_out = false;

    loop {
        let response = tokio::select! {
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
            response = rx.recv() => response,
        };
        // Nothing else to receive.
        let Some(response) = response else { break };

        let mut result = timeout(write_timeout, ws_sender.send_text_owned(response)).await;
        // Write the messages that are already queued too, and flush them together.
        let mut written = 1;
        while matches!(result, Ok(Ok(()))) && written < MAX_MESSAGES_PER_FLUSH {
            let Ok(response) = rx.try_recv() else { break };
            result = timeout(write_timeout, ws_sender.send_text_owned(response)).await;
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
        let _ = timeout(write_timeout, ws_sender.close()).await;
    }
}

enum Receive<S> {
    ConnectionClosed,
    Stopped,
    Err(SokettoError, S),
    Ok(Vec<u8>, S),
}

/// Attempts to read data from WebSocket fails if the server was stopped.
async fn try_recv<T, S>(ws_stream: &mut T, stopped: S) -> Receive<S>
where
    S: Future<Output = ()> + Unpin,
    T: StreamExt<Item = Result<Vec<u8>, SokettoError>> + Unpin,
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
    result: Result<Shutdown, SokettoError>,
    pending_calls: mpsc::Receiver<()>,
    ws_stream: S,
    mut conn_tx: oneshot::Sender<()>,
) where
    S: StreamExt<Item = Result<Vec<u8>, SokettoError>> + Unpin,
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
        let mut always_ready = stream::repeat_with(|| Ok::<_, SokettoError>(b"{}".to_vec()));

        let received = try_recv(&mut always_ready, future::ready(())).await;
        assert!(matches!(received, Receive::Stopped));
    }
}
