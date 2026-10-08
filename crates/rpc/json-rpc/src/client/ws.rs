use super::{BoxError, Client, ClientBuilder, Error};
use bytes::Bytes;
use futures_util::{stream, Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Interval};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::HeaderMap,
    protocol::{Message, WebSocketConfig},
    Error as WsError,
};

/// Builds a `WebSocket` [`Client`].
#[derive(Clone, Debug)]
pub struct WsClientBuilder {
    headers: HeaderMap,
    client: ClientBuilder,
    max_message_size: Option<usize>,
    connection_timeout: Duration,
    ping: Option<PingConfig>,
}

impl Default for WsClientBuilder {
    fn default() -> Self {
        Self {
            headers: HeaderMap::new(),
            client: ClientBuilder::default(),
            max_message_size: None,
            connection_timeout: Duration::from_secs(10),
            ping: None,
        }
    }
}

impl WsClientBuilder {
    /// Sets headers sent with the handshake request.
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Sets the request timeout. Default is 60 seconds.
    pub const fn request_timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.request_timeout(timeout);
        self
    }

    /// Sets the maximum size of a received message. Default is 64 MiB.
    pub const fn max_message_size(mut self, size: usize) -> Self {
        self.max_message_size = Some(size);
        self
    }

    /// Sets the timeout for connecting and completing the handshake. Default is 10 seconds.
    pub const fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }

    /// See [`ClientBuilder::max_concurrent_requests`].
    pub const fn max_concurrent_requests(mut self, max: usize) -> Self {
        self.client = self.client.max_concurrent_requests(max);
        self
    }

    /// See [`ClientBuilder::max_buffer_capacity_per_subscription`].
    pub const fn max_buffer_capacity_per_subscription(mut self, capacity: usize) -> Self {
        self.client = self.client.max_buffer_capacity_per_subscription(capacity);
        self
    }

    /// Sends pings and closes the connection once the server stops answering. Disabled by
    /// default.
    pub const fn enable_ws_ping(mut self, config: PingConfig) -> Self {
        self.ping = Some(config);
        self
    }

    /// Disables pings.
    pub const fn disable_ws_ping(mut self) -> Self {
        self.ping = None;
        self
    }

    /// Connects to the given URL.
    pub async fn build(self, url: impl AsRef<str>) -> Result<Client, Error> {
        let mut request =
            url.as_ref().into_client_request().map_err(|err| Error::Transport(err.into()))?;
        request.headers_mut().extend(self.headers);
        let config = WebSocketConfig::default()
            .max_message_size(self.max_message_size)
            .max_frame_size(self.max_message_size);
        let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), true);
        let (ws, _) = tokio::time::timeout(self.connection_timeout, connect)
            .await
            .map_err(|_| Error::Transport("connection timed out".into()))?
            .map_err(|err| Error::Transport(err.into()))?;
        let (sink, stream) = ws.split();
        let mut pings = self.ping.map(|ping| tokio::time::interval(ping.ping_interval));
        let pings = stream::poll_fn(move |cx| match &mut pings {
            Some(pings) => pings.poll_tick(cx).map(|_| Some(Message::Ping(Bytes::new()))),
            None => Poll::Pending,
        });
        let reader = Reader { stream, heartbeat: self.ping.map(Heartbeat::new) };
        Ok(self.client.build_with_keepalive(reader, sink, pings))
    }
}

/// A `WebSocket` client.
pub type WsClient = Client;

/// Configures `WebSocket` pings that detect an unresponsive server.
///
/// The connection is closed once no message, including pongs, was received for `inactive_limit`,
/// `max_failures` times in a row.
#[derive(Clone, Copy, Debug)]
pub struct PingConfig {
    ping_interval: Duration,
    inactive_limit: Duration,
    max_failures: usize,
}

impl Default for PingConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl PingConfig {
    /// Creates a config that pings every 30 seconds and closes the connection after 40 seconds
    /// of inactivity.
    pub const fn new() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            inactive_limit: Duration::from_secs(40),
            max_failures: 1,
        }
    }

    /// Sets the interval between pings. Default is 30 seconds.
    pub const fn ping_interval(mut self, interval: Duration) -> Self {
        self.ping_interval = interval;
        self
    }

    /// Sets how long the server may stay silent before it counts as a failure. Default is 40
    /// seconds.
    ///
    /// It should be longer than the ping interval.
    pub const fn inactive_limit(mut self, limit: Duration) -> Self {
        self.inactive_limit = limit;
        self
    }

    /// Sets the number of consecutive failures that close the connection. Default is 1.
    pub const fn max_failures(mut self, max: usize) -> Self {
        self.max_failures = max;
        self
    }
}

/// Reads JSON-RPC messages, failing once the server was inactive for too long.
struct Reader<S> {
    stream: S,
    heartbeat: Option<Heartbeat>,
}

impl<S: Stream<Item = Result<Message, WsError>> + Unpin> Stream for Reader<S> {
    type Item = Result<Bytes, BoxError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        while let Poll::Ready(msg) = this.stream.poll_next_unpin(cx) {
            let msg = match msg {
                Some(Ok(msg)) => msg,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => return Poll::Ready(None),
            };
            if let Some(heartbeat) = &mut this.heartbeat {
                heartbeat.last_active = Instant::now();
                heartbeat.failures = 0;
            }
            match msg {
                Message::Text(text) => return Poll::Ready(Some(Ok(text.into()))),
                Message::Binary(bytes) => return Poll::Ready(Some(Ok(bytes))),
                _ => {}
            }
        }
        if let Some(heartbeat) = &mut this.heartbeat {
            while heartbeat.checks.poll_tick(cx).is_ready() {
                if heartbeat.last_active.elapsed() >= heartbeat.inactive_limit {
                    heartbeat.failures += 1;
                    if heartbeat.failures >= heartbeat.max_failures {
                        return Poll::Ready(Some(Err("server stopped answering pings".into())))
                    }
                }
            }
        }
        Poll::Pending
    }
}

struct Heartbeat {
    checks: Interval,
    inactive_limit: Duration,
    max_failures: usize,
    last_active: Instant,
    failures: usize,
}

impl Heartbeat {
    fn new(config: PingConfig) -> Self {
        let now = Instant::now();
        Self {
            checks: tokio::time::interval_at(now + config.inactive_limit, config.inactive_limit),
            inactive_limit: config.inactive_limit,
            max_failures: config.max_failures,
            last_active: now,
            failures: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientT;
    use tokio::net::TcpListener;

    /// Accepts one `WebSocket` connection, reading from it only if `answer` is set.
    async fn serve(answer: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            if answer {
                while ws.next().await.is_some() {}
            } else {
                std::future::pending::<()>().await;
            }
        });
        format!("ws://{addr}")
    }

    #[tokio::test]
    async fn connection_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let err = WsClientBuilder::default()
            .connection_timeout(Duration::from_millis(50))
            .build(url)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "transport error: connection timed out");
    }

    #[tokio::test]
    async fn ping() {
        let ping = PingConfig::new()
            .ping_interval(Duration::from_millis(20))
            .inactive_limit(Duration::from_millis(50))
            .max_failures(2);

        let client = WsClientBuilder::default().enable_ws_ping(ping).build(serve(true).await);
        let client = client.await.unwrap();
        let disconnect = tokio::time::timeout(Duration::from_millis(300), client.on_disconnect());
        assert!(disconnect.await.is_err());

        let client = WsClientBuilder::default().enable_ws_ping(ping).build(serve(false).await);
        let client = client.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), client.on_disconnect()).await.unwrap();
        let err = client.request::<u64, _>("m", crate::rpc_params![]).await.unwrap_err();
        assert!(matches!(err, Error::Closed));

        let client = WsClientBuilder::default()
            .enable_ws_ping(ping)
            .disable_ws_ping()
            .build(serve(false).await)
            .await
            .unwrap();
        let disconnect = tokio::time::timeout(Duration::from_millis(300), client.on_disconnect());
        assert!(disconnect.await.is_err());
    }
}
