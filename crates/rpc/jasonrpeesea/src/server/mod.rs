//! HTTP and `WebSocket` server.

use crate::{
    connection::handle_message, error::exceeded_limit, serve_connection, stop_channel, Id,
    MethodResponse, Methods, RpcService, RpcServiceBuilder, RpcServiceT, ServerConfig,
    ServerHandle, StopHandle, OVERSIZED_REQUEST_CODE, OVERSIZED_REQUEST_MSG,
};
use bytes::Bytes;
use futures_util::{future::BoxFuture, SinkExt, StreamExt};
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
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, ToSocketAddrs},
    sync::{OwnedSemaphorePermit, Semaphore},
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
        Ok(Server {
            listener,
            config: self.config,
            http_middleware: self.http_middleware,
            rpc_middleware: self.rpc_middleware,
        })
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
    pub fn start(self, methods: impl Into<Methods>) -> ServerHandle {
        let (stop, handle) = stop_channel();
        let config = self.config.clone();
        config.spawn(self.run(methods.into(), stop));
        handle
    }

    async fn run(self, methods: Methods, stop: StopHandle) {
        let Self { listener, config, http_middleware, rpc_middleware } = self;
        let connections = Arc::new(Semaphore::new(config.max_connections as usize));
        let max_response_size = config.max_response_body_size as usize;
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
            let Ok(permit) = connections.clone().try_acquire_owned() else {
                tracing::debug!(target: "rpc::jsonrpc", "too many connections");
                continue
            };
            let _ = socket.set_nodelay(true);
            let rpc = shared.rpc_middleware.service(RpcService::new(
                shared.methods.clone(),
                max_response_size,
                None,
            ));
            let service = http_middleware.service(TowerService {
                shared: shared.clone(),
                rpc: Arc::new(rpc),
                stop: stop.clone(),
                _permit: Arc::new(permit),
            });
            let stopped = stop.clone().shutdown();
            tokio::spawn(async move {
                if let Err(err) = serve_with_graceful_shutdown(socket, service, stopped).await {
                    tracing::debug!(target: "rpc::jsonrpc", %err, "connection failed");
                }
            });
        }
    }
}

struct Shared<RL> {
    methods: Methods,
    rpc_middleware: RpcServiceBuilder<RL>,
    config: ServerConfig,
}

/// The innermost HTTP service of a connection, which handles JSON-RPC requests and `WebSocket`
/// handshakes.
pub struct TowerService<RL: Layer<RpcService>> {
    shared: Arc<Shared<RL>>,
    rpc: Arc<RL::Service>,
    stop: StopHandle,
    _permit: Arc<OwnedSemaphorePermit>,
}

impl<RL: Layer<RpcService>> Clone for TowerService<RL> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
            rpc: self.rpc.clone(),
            stop: self.stop.clone(),
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

    fn call(&mut self, req: HttpRequest) -> Self::Future {
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

        let max_request_size = config.max_request_body_size as usize;
        let body = match Limited::new(req.into_body(), max_request_size).collect().await {
            Ok(body) => body.to_bytes(),
            Err(err) if err.is::<LengthLimitError>() => {
                let err =
                    exceeded_limit(OVERSIZED_REQUEST_CODE, OVERSIZED_REQUEST_MSG, max_request_size);
                return json_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    MethodResponse::error(Id::Null, err).into_json(),
                );
            }
            Err(_) => return status_response(StatusCode::BAD_REQUEST),
        };
        let max_response_size = config.max_response_body_size as usize;
        match handle_message(&*self.rpc, body, max_response_size).await {
            Some(json) => json_response(StatusCode::OK, json),
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
            let reader = stream
                .scan((), |(), msg| {
                    ready(match msg {
                        Ok(Message::Text(text)) => Some(Some(Bytes::from(text))),
                        Ok(Message::Binary(bytes)) => Some(Some(bytes)),
                        Ok(Message::Close(_)) | Err(_) => None,
                        Ok(_) => Some(None),
                    })
                })
                .filter_map(ready);
            let writer = sink.with(|msg: String| ready(Ok::<_, WsError>(Message::text(msg))));
            serve_connection(
                pin!(reader),
                pin!(writer),
                methods.clone(),
                rpc_middleware,
                config,
                self.stop.clone(),
            )
            .await;
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
    let service = TowerToHyperService::new(
        service.map_request(|req: http::Request<Incoming>| req.map(HttpBody::new)),
    );
    let builder = auto::Builder::new(TokioExecutor::new());
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
        rpc_params, RpcModule, SubscriptionMessage,
    };

    async fn start(config: ServerConfig) -> (SocketAddr, ServerHandle) {
        let mut module = RpcModule::new(());
        module.register_method("echo", |params, _| params.one::<String>()).unwrap();
        module
            .register_subscription("sub", "notif", "unsub", |_, pending, _| async move {
                let sink = pending.accept().await?;
                let msg = SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &1)?;
                sink.send(msg).await?;
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
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.stopped()).await.unwrap();
    }

    #[tokio::test]
    async fn http_only_and_max_connections() {
        let (addr, _handle) = start(ServerConfig::default().http_only().max_connections(1)).await;
        assert!(WsClientBuilder::default().build(format!("ws://{addr}")).await.is_err());
        let http = HttpClientBuilder::default().build(format!("http://{addr}")).unwrap();
        assert_eq!(http.request::<String, _>("echo", rpc_params!["a"]).await.unwrap(), "a");
    }
}
