//! HTTP and `WebSocket` server.

use crate::{
    connection::handle_message, reject_too_big_request, serve_connection, stop_channel,
    ConnectionId, Id, MethodResponse, PingConfig, RpcModule, RpcService, RpcServiceBuilder,
    RpcServiceT, ServerConfig, ServerHandle, StopHandle,
};
use bytes::Bytes;
use futures_util::{
    future::{BoxFuture, Either},
    sink, SinkExt, StreamExt,
};
use http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use http_body::Body;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
    service::TowerToHyperService,
};
use std::{
    future::{ready, Future},
    io,
    net::SocketAddr,
    pin::pin,
    sync::{Arc, Mutex as StdMutex},
    task::{Context, Poll},
    time::Instant,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, ToSocketAddrs},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_tungstenite::{
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Message, Role, WebSocketConfig},
        Error as WsError,
    },
    WebSocketStream,
};
use tower::{layer::util::Identity, BoxError, Layer, Service, ServiceBuilder, ServiceExt};

mod body;
pub use body::{HttpBody, HttpRequest, HttpResponse};

/// Builds an HTTP and `WebSocket` [`Server`].
#[derive(Clone, Debug)]
pub struct ServerBuilder<HL = Identity, RL = Identity> {
    config: ServerConfig,
    http_middleware: ServiceBuilder<HL>,
    rpc_middleware: RpcServiceBuilder<RL>,
}

impl ServerBuilder {
    /// Creates a builder with the default configuration and no middleware.
    pub fn new() -> Self {
        Self {
            config: ServerConfig::default(),
            http_middleware: ServiceBuilder::new(),
            rpc_middleware: RpcServiceBuilder::new(),
        }
    }
}

impl Default for ServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl<HL, RL> ServerBuilder<HL, RL> {
    /// Sets the configuration.
    pub fn set_config(mut self, config: ServerConfig) -> Self {
        self.config = config;
        self
    }

    /// Sets the HTTP middleware, which also applies to `WebSocket` handshakes.
    pub fn set_http_middleware<T>(
        self,
        http_middleware: ServiceBuilder<T>,
    ) -> ServerBuilder<T, RL> {
        ServerBuilder { config: self.config, http_middleware, rpc_middleware: self.rpc_middleware }
    }

    /// Sets the RPC middleware.
    pub fn set_rpc_middleware<T>(
        self,
        rpc_middleware: RpcServiceBuilder<T>,
    ) -> ServerBuilder<HL, T> {
        ServerBuilder { config: self.config, http_middleware: self.http_middleware, rpc_middleware }
    }

    /// Binds the server to the given address.
    pub async fn build(self, addr: impl ToSocketAddrs) -> io::Result<Server<HL, RL>> {
        let listener = TcpListener::bind(addr).await?;
        Ok(self.build_from_listener(listener))
    }

    /// Builds the server from an already bound standard library listener.
    pub fn build_from_tcp(self, listener: std::net::TcpListener) -> io::Result<Server<HL, RL>> {
        listener.set_nonblocking(true)?;
        Ok(self.build_from_listener(TcpListener::from_std(listener)?))
    }

    fn build_from_listener(self, listener: TcpListener) -> Server<HL, RL> {
        Server {
            listener,
            config: self.config,
            http_middleware: self.http_middleware,
            rpc_middleware: self.rpc_middleware,
        }
    }
}

/// A bound HTTP and `WebSocket` server.
#[derive(Debug)]
pub struct Server<HL = Identity, RL = Identity> {
    listener: TcpListener,
    config: ServerConfig,
    http_middleware: ServiceBuilder<HL>,
    rpc_middleware: RpcServiceBuilder<RL>,
}

impl<HL, RL> Server<HL, RL> {
    /// Returns the bound address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

impl<HL, RL> Server<HL, RL>
where
    RL: Layer<RpcService> + Send + Sync + 'static,
    RL::Service: RpcServiceT + 'static,
    HL: Layer<TowerService<RL>> + Send + 'static,
    HL::Service: Service<HttpRequest, Response = HttpResponse> + Clone + Send + 'static,
    <HL::Service as Service<HttpRequest>>::Future: Send,
    <HL::Service as Service<HttpRequest>>::Error: Into<BoxError>,
{
    /// Starts serving the given methods.
    ///
    /// The server stops when [`ServerHandle::stop`] is called or every handle is dropped.
    pub fn start(self, methods: RpcModule) -> ServerHandle {
        let (stop, handle) = stop_channel();
        let config = self.config.clone();
        config.spawn(self.run(methods, stop));
        handle
    }

    async fn run(self, methods: RpcModule, stop: StopHandle) {
        let Self { listener, config, http_middleware, rpc_middleware } = self;
        let connections = Arc::new(Semaphore::new(config.max_connections as usize));
        let max_response_size = config.max_response_body_size as usize;
        let keep_alive = config.keep_alive;
        let tcp_no_delay = config.tcp_no_delay;
        let shared = Arc::new(Shared { methods, rpc_middleware, config });
        let mut stopped = pin!(stop.clone().shutdown());
        loop {
            let socket = tokio::select! {
                res = listener.accept() => match res {
                    Ok((socket, _)) => socket,
                    Err(err) => {
                        tracing::debug!(target: "rpc::jsonrpc", %err, "failed to accept connection");
                        continue
                    }
                },
                () = &mut stopped => break,
            };
            let _ = socket.set_nodelay(tcp_no_delay);
            let stopped = stop.clone().shutdown();
            let Ok(permit) = connections.clone().try_acquire_owned() else {
                tracing::debug!(target: "rpc::jsonrpc", "too many connections");
                let service = tower::service_fn(|_| {
                    ready(Ok::<_, BoxError>(status_response(StatusCode::TOO_MANY_REQUESTS)))
                });
                tokio::spawn(serve_http(socket, service, stopped, false));
                continue
            };
            let rpc = shared.rpc_middleware.service(RpcService::new(
                shared.methods.clone(),
                max_response_size,
                None,
            ));
            let service = http_middleware.service(TowerService {
                shared: shared.clone(),
                rpc: Arc::new(rpc),
                stop: stop.clone(),
                conn_id: ConnectionId::next(),
                _permit: Arc::new(permit),
            });
            tokio::spawn(serve_http(socket, service, stopped, keep_alive));
        }
    }
}

struct Shared<RL> {
    methods: RpcModule,
    rpc_middleware: RpcServiceBuilder<RL>,
    config: ServerConfig,
}

/// The innermost HTTP service of a connection, which handles JSON-RPC requests and `WebSocket`
/// handshakes.
pub struct TowerService<RL: Layer<RpcService>> {
    shared: Arc<Shared<RL>>,
    rpc: Arc<RL::Service>,
    stop: StopHandle,
    conn_id: ConnectionId,
    _permit: Arc<OwnedSemaphorePermit>,
}

impl<RL: Layer<RpcService>> Clone for TowerService<RL> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            rpc: self.rpc.clone(),
            stop: self.stop.clone(),
            conn_id: self.conn_id,
            _permit: self._permit.clone(),
        }
    }
}

impl<RL: Layer<RpcService>> std::fmt::Debug for TowerService<RL> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TowerService").finish_non_exhaustive()
    }
}

impl<RL> Service<HttpRequest> for TowerService<RL>
where
    RL: Layer<RpcService> + Send + Sync + 'static,
    RL::Service: RpcServiceT + 'static,
{
    type Response = HttpResponse;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<HttpResponse, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: HttpRequest) -> Self::Future {
        req.extensions_mut().insert(self.conn_id);
        let this = self.clone();
        Box::pin(async move { Ok(this.handle(req).await) })
    }
}

impl<RL> TowerService<RL>
where
    RL: Layer<RpcService> + Send + Sync + 'static,
    RL::Service: RpcServiceT + 'static,
{
    async fn handle(self, req: HttpRequest) -> HttpResponse {
        let config = &self.shared.config;
        if config.enable_ws && is_upgrade_request(&req) {
            return self.upgrade(req)
        }
        if !config.enable_http {
            return status_response(StatusCode::FORBIDDEN)
        }
        if req.method() != Method::POST {
            return status_response(StatusCode::METHOD_NOT_ALLOWED)
        }
        if !is_json(req.headers()) {
            return status_response(StatusCode::UNSUPPORTED_MEDIA_TYPE)
        }

        let extensions = req.extensions().clone();
        let max_request_size = config.max_request_body_size as usize;
        let body = match Limited::new(req.into_body(), max_request_size).collect().await {
            Ok(body) => body.to_bytes(),
            Err(err) if err.is::<LengthLimitError>() => {
                let err = reject_too_big_request(max_request_size);
                return json_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    MethodResponse::error(Id::Null, err).into_json(),
                );
            }
            Err(_) => return status_response(StatusCode::BAD_REQUEST),
        };
        let max_response_size = config.max_response_body_size as usize;
        match handle_message(&*self.rpc, body, max_response_size, config.batch_config, &extensions)
            .await
        {
            Some((json, _)) => json_response(StatusCode::OK, json),
            None => status_response(StatusCode::OK),
        }
    }

    /// Accepts a `WebSocket` handshake and serves the connection in a new task.
    fn upgrade(self, mut req: HttpRequest) -> HttpResponse {
        let Some(key) = req.headers().get(header::SEC_WEBSOCKET_KEY) else {
            return status_response(StatusCode::BAD_REQUEST)
        };
        let accept = derive_accept_key(key.as_bytes());
        let on_upgrade = hyper::upgrade::on(&mut req);
        let extensions = req.extensions().clone();
        tokio::spawn(async move {
            let upgraded = match on_upgrade.await {
                Ok(upgraded) => upgraded,
                Err(err) => {
                    tracing::debug!(target: "rpc::jsonrpc", %err, "websocket upgrade failed");
                    return
                }
            };
            let Shared { methods, rpc_middleware, config } = &*self.shared;
            let max_size = config.max_request_body_size as usize;
            let ws_config = WebSocketConfig::default()
                .max_message_size(Some(max_size))
                .max_frame_size(Some(max_size));
            let ws = WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                Some(ws_config),
            )
            .await;
            let (sink, stream) = ws.split();
            let last_active = config.ping_config.map(|_| Arc::new(StdMutex::new(Instant::now())));
            let active = last_active.clone();
            let reader = stream
                .scan((), move |(), msg| {
                    if let Some(active) = &active &&
                        let Ok(mut active) = active.lock()
                    {
                        *active = Instant::now();
                    }
                    ready(match msg {
                        Ok(Message::Text(text)) => Some(Some(Bytes::from(text))),
                        Ok(Message::Binary(bytes)) => Some(Some(bytes)),
                        Ok(Message::Close(_)) | Err(_) => None,
                        Ok(_) => Some(None),
                    })
                })
                .filter_map(ready);
            let text = |msg: String| Message::text(msg);
            let (reader, writer, ping) = match (config.ping_config, last_active) {
                (Some(ping_config), Some(last_active)) => {
                    let sink = Arc::new(Mutex::new(sink));
                    let ping = tokio::spawn(ping(sink.clone(), ping_config, last_active));
                    let abort = ping.abort_handle();
                    let writer = sink::unfold(sink, move |sink, msg| async move {
                        let res = sink.lock().await.send(text(msg)).await;
                        res.map(|()| sink)
                    });
                    (Either::Left(reader.take_until(ping)), Either::Left(writer), Some(abort))
                }
                _ => {
                    let writer = sink.with(move |msg| ready(Ok::<_, WsError>(text(msg))));
                    (Either::Right(reader), Either::Right(writer), None)
                }
            };
            serve_connection(
                pin!(reader),
                pin!(writer),
                methods.clone(),
                rpc_middleware,
                config,
                self.stop.clone(),
                extensions,
            )
            .await;
            if let Some(ping) = ping {
                ping.abort();
            }
        });

        let mut response = status_response(StatusCode::SWITCHING_PROTOCOLS);
        let headers = response.headers_mut();
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        if let Ok(accept) = HeaderValue::from_str(&accept) {
            headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
        }
        response
    }
}

/// Serves an HTTP/1 or HTTP/2 connection until it closes, or until `stopped` resolves and the
/// in-flight requests complete.
pub async fn serve_with_graceful_shutdown<I, S, B>(
    io: I,
    service: S,
    stopped: impl Future<Output = ()>,
) -> Result<(), BoxError>
where
    I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    S: Service<HttpRequest, Response = http::Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    serve_with_keep_alive(io, service, stopped, true).await
}

async fn serve_http<I, S, B>(io: I, service: S, stopped: impl Future<Output = ()>, keep_alive: bool)
where
    I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    S: Service<HttpRequest, Response = http::Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    if let Err(err) = serve_with_keep_alive(io, service, stopped, keep_alive).await {
        tracing::debug!(target: "rpc::jsonrpc", %err, "connection failed");
    }
}

async fn serve_with_keep_alive<I, S, B>(
    io: I,
    service: S,
    stopped: impl Future<Output = ()>,
    keep_alive: bool,
) -> Result<(), BoxError>
where
    I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    S: Service<HttpRequest, Response = http::Response<B>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError>,
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Into<BoxError>,
{
    let service = TowerToHyperService::new(
        service.map_request(|req: http::Request<Incoming>| req.map(HttpBody::new)),
    );
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().keep_alive(keep_alive);
    let mut conn = pin!(builder.serve_connection_with_upgrades(TokioIo::new(io), service));
    let mut stopped = pin!(stopped);
    tokio::select! {
        res = conn.as_mut() => res,
        () = &mut stopped => {
            conn.as_mut().graceful_shutdown();
            conn.await
        }
    }
}

/// Pings the `WebSocket` peer until it stops answering or a ping fails to send.
async fn ping<S>(sink: Arc<Mutex<S>>, config: PingConfig, last_active: Arc<StdMutex<Instant>>)
where
    S: futures_util::Sink<Message> + Unpin,
{
    let mut interval = tokio::time::interval(config.ping_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    let mut failures = 0;
    loop {
        interval.tick().await;
        let idle =
            last_active.lock().map_or_else(|_| Default::default(), |active| active.elapsed());
        if idle > config.inactive_limit {
            failures += 1;
            if failures >= config.max_failures {
                tracing::debug!(target: "rpc::jsonrpc", "websocket peer stopped answering pings");
                return
            }
        } else {
            failures = 0;
        }
        if sink.lock().await.send(Message::Ping(Bytes::new())).await.is_err() {
            return
        }
    }
}

fn is_upgrade_request<B>(req: &http::Request<B>) -> bool {
    let has_token = |name, token: &str| {
        req.headers().get_all(name).iter().any(|value| {
            value
                .to_str()
                .is_ok_and(|value| value.split(',').any(|v| v.trim().eq_ignore_ascii_case(token)))
        })
    };
    req.method() == Method::GET &&
        has_token(header::CONNECTION, "upgrade") &&
        has_token(header::UPGRADE, "websocket")
}

fn is_json(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| {
        let mime = v.split(';').next().unwrap_or_default().trim();
        ["application/json", "application/json-rpc", "application/jsonrequest"]
            .iter()
            .any(|m| mime.eq_ignore_ascii_case(m))
    })
}

fn status_response(status: StatusCode) -> HttpResponse {
    let mut response = HttpResponse::new(HttpBody::empty());
    *response.status_mut() = status;
    response
}

fn json_response(status: StatusCode, json: String) -> HttpResponse {
    let mut response = HttpResponse::new(HttpBody::from(json));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    response
}

#[cfg(all(test, feature = "http-client", feature = "ws-client"))]
mod tests {
    use super::*;
    use crate::{
        client::{ClientT, Error, HttpClientBuilder, SubscriptionClientT, WsClientBuilder},
        rpc_params, OVERSIZED_REQUEST_CODE,
    };
    use std::time::Duration;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };

    async fn start(config: ServerConfig) -> (SocketAddr, ServerHandle) {
        let mut module = RpcModule::new();
        module.register_method("echo", |params, _| params.one::<String>()).unwrap();
        module
            .register_subscription("sub", "notif", "unsub", |_, pending, _| async move {
                let sink = pending.accept().await?;
                sink.send(&1).await?;
                sink.closed().await;
                Ok(())
            })
            .unwrap();
        let server = ServerBuilder::new().set_config(config).build("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        (addr, server.start(module))
    }

    #[tokio::test]
    async fn http_and_ws() {
        let (addr, handle) = start(ServerConfig::default().max_request_body_size(100)).await;

        let http = HttpClientBuilder::default().build(format!("http://{addr}")).unwrap();
        assert_eq!(http.request::<String, _>("echo", rpc_params!["a"]).await.unwrap(), "a");
        let err =
            http.request::<String, _>("echo", rpc_params!["a".repeat(100)]).await.unwrap_err();
        assert!(matches!(err, Error::Call(err) if err.code() == OVERSIZED_REQUEST_CODE));
        let err = http.subscribe::<u64, _>("sub", rpc_params![], "unsub").await.unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));

        let ws = WsClientBuilder::default().build(format!("ws://{addr}")).await.unwrap();
        assert_eq!(ws.request::<String, _>("echo", rpc_params!["b"]).await.unwrap(), "b");
        let mut sub = ws.subscribe::<u64, _>("sub", rpc_params![], "unsub").await.unwrap();
        assert_eq!(sub.next().await.unwrap().unwrap(), 1);
        sub.unsubscribe().await.unwrap();

        handle.stop().unwrap();
        tokio::time::timeout(Duration::from_secs(5), handle.stopped()).await.unwrap();
    }

    #[tokio::test]
    async fn http_only_and_max_connections() {
        let (addr, _handle) = start(ServerConfig::default().http_only().max_connections(1)).await;
        assert!(WsClientBuilder::default().build(format!("ws://{addr}")).await.is_err());
        let http = HttpClientBuilder::default().build(format!("http://{addr}")).unwrap();
        assert_eq!(http.request::<String, _>("echo", rpc_params!["a"]).await.unwrap(), "a");
    }

    /// Sends a raw HTTP request on `stream` and returns the response head.
    async fn raw_http(stream: &mut TcpStream) -> String {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"echo","params":["a"]}"#;
        let req = format!(
            "POST / HTTP/1.1\r\nhost: x\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = vec![0; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf[..n]).lines().next().unwrap().to_owned()
    }

    #[tokio::test]
    async fn too_many_connections() {
        let (addr, _handle) = start(ServerConfig::default().max_connections(1)).await;
        let mut first = TcpStream::connect(addr).await.unwrap();
        assert_eq!(raw_http(&mut first).await, "HTTP/1.1 200 OK");
        let mut second = TcpStream::connect(addr).await.unwrap();
        assert_eq!(raw_http(&mut second).await, "HTTP/1.1 429 Too Many Requests");
        assert_eq!(raw_http(&mut first).await, "HTTP/1.1 200 OK");
    }

    #[tokio::test]
    async fn ws_ping() {
        let ping = PingConfig::new()
            .ping_interval(Duration::from_millis(20))
            .inactive_limit(Duration::from_millis(50));
        let (addr, _handle) = start(ServerConfig::default().enable_ws_ping(ping)).await;
        let url = format!("ws://{addr}");

        // A peer that reads answers pings and stays connected.
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let mut pings = 0;
        while pings < 10 {
            match ws.next().await.unwrap().unwrap() {
                Message::Ping(_) => pings += 1,
                msg => panic!("unexpected message: {msg:?}"),
            }
        }
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"echo","params":["a"]}"#;
        ws.send(Message::text(req)).await.unwrap();
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Ping(_) => {}
                msg => {
                    assert_eq!(msg.to_text().unwrap(), r#"{"jsonrpc":"2.0","id":1,"result":"a"}"#);
                    break
                }
            }
        }

        // A peer that does not read sends no pongs and gets disconnected.
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_close() {
                    break
                }
            }
        });
        closed.await.unwrap();
    }
}
