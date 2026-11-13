use crate::{
    server::{ConnectionState, ServerConfig},
    transport::stream::{serve_stream, MessageWriter, StreamConnection, TooLarge},
    LOG_TARGET,
};

use futures_util::io::{BufReader, BufWriter};
use hyper::upgrade::Upgraded;
use hyper_util::rt::TokioIo;
use jsonrpsee_core::{middleware::RpcServiceT, server::MethodSink};
use soketto::{connection::Error as SokettoError, data::ByteSlice125};
use std::{future::Future, io, time::Duration};
use tokio::{sync::mpsc, time::timeout};
use tokio_util::compat::Compat;

pub(crate) type Sender = soketto::Sender<BufReader<BufWriter<Compat<TokioIo<Upgraded>>>>>;
pub(crate) type Receiver = soketto::Receiver<BufReader<BufWriter<Compat<TokioIo<Upgraded>>>>>;

pub use soketto::handshake::http::is_upgrade_request;

pub(crate) struct BackgroundTaskParams<S> {
    pub(crate) server_cfg: ServerConfig,
    pub(crate) conn: ConnectionState,
    pub(crate) ws_sender: Sender,
    pub(crate) ws_receiver: Receiver,
    pub(crate) rpc_service: S,
    pub(crate) sink: MethodSink,
    pub(crate) rx: mpsc::Receiver<String>,
}

pub(crate) async fn background_task<S>(params: BackgroundTaskParams<S>)
where
    S: RpcServiceT + Send + Sync + 'static,
{
    let BackgroundTaskParams { server_cfg, conn, ws_sender, ws_receiver, rpc_service, sink, rx } =
        params;
    let ServerConfig {
        batch_requests_config,
        max_request_body_size,
        write_timeout,
        message_buffer_capacity,
        ping_interval,
        ..
    } = server_cfg;

    let conn_id = conn.conn_id;
    // `soketto` allocates a frame's announced length before reading its payload, so a frame that
    // never completes would hold that buffer for good. A pong can't arrive inside a frame, so a
    // read that yields neither a message nor a pong to the pings within two intervals is stuck.
    let read_timeout = ping_interval * 2;
    let incoming = futures_util::stream::unfold(ws_receiver, move |mut receiver| async move {
        let mut data = Vec::new();
        loop {
            let received = timeout(read_timeout, receiver.receive(&mut data))
                .await
                .unwrap_or_else(|_| Err(SokettoError::Io(io::ErrorKind::TimedOut.into())));
            match received {
                Ok(soketto::Incoming::Data(_)) => return Some((Ok(data), receiver)),
                // Pongs only answer the server's pings, which restarts the read timeout.
                Ok(soketto::Incoming::Pong(_)) => data.clear(),
                Ok(soketto::Incoming::Closed(_)) | Err(SokettoError::Closed) => return None,
                Err(SokettoError::MessageTooLarge { current, maximum }) => {
                    tracing::debug!(
                        target: LOG_TARGET,
                        "WS recv error: message too large current={}/max={}",
                        current,
                        maximum
                    );
                    return Some((Err(TooLarge), receiver));
                }
                Err(e) => {
                    tracing::debug!(target: LOG_TARGET, "WS error: {}; terminate connection: {}", e, conn_id);
                    return None;
                }
            }
        }
    });
    tokio::pin!(incoming);

    serve_stream(StreamConnection {
        incoming,
        outgoing: WsWriter { sender: ws_sender, write_timeout, timed_out: false },
        rpc_service,
        sink,
        rx,
        stopped: conn.stop_handle.clone().shutdown(),
        batch_requests_config,
        max_request_body_size,
        max_concurrent_calls: message_buffer_capacity as usize,
        ping_interval: Some(ping_interval),
    })
    .await;

    drop(conn);
}

/// Writes text messages and pings to a WebSocket.
struct WsWriter {
    sender: Sender,
    /// How long a single write, a flush or the close may take.
    write_timeout: Duration,
    /// Set once a write timed out: the client isn't reading, so nothing more is written.
    timed_out: bool,
}

impl WsWriter {
    /// Runs a write for at most the write timeout.
    async fn bounded(
        write_timeout: Duration,
        timed_out: &mut bool,
        write: impl Future<Output = Result<(), SokettoError>>,
    ) -> Result<(), SokettoError> {
        timeout(write_timeout, write).await.unwrap_or_else(|_| {
            *timed_out = true;
            Err(SokettoError::Io(io::ErrorKind::TimedOut.into()))
        })
    }
}

impl MessageWriter for WsWriter {
    type Error = SokettoError;

    async fn write(&mut self, message: String) -> Result<(), Self::Error> {
        let write = self.sender.send_text_owned(message);
        Self::bounded(self.write_timeout, &mut self.timed_out, write).await
    }

    async fn ping(&mut self) -> Result<(), Self::Error> {
        let payload = ByteSlice125::try_from(&[][..]).expect("empty ping payload fits");
        let write = self.sender.send_ping(payload);
        Self::bounded(self.write_timeout, &mut self.timed_out, write).await
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        let write = self.sender.flush();
        Self::bounded(self.write_timeout, &mut self.timed_out, write).await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        // The close frame would block on the same full socket as the write that timed out.
        if self.timed_out {
            return Ok(());
        }
        let write = self.sender.close();
        Self::bounded(self.write_timeout, &mut self.timed_out, write).await
    }
}
