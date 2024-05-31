use std::{fmt, net::SocketAddr};

use reth_ipc::server::IpcServer;
use reth_rpc_layer::JwtSecret;
use tower::layer::util::{Identity, Stack};

use crate::{error::RpcError, metrics::RpcRequestMetrics};

/// Container type for each transport ie. http, ws, and ipc server
pub struct RpcServer {
    /// Configured ws,http servers
    ws_http: WsHttpServer,
    /// ipc server
    ipc: Option<IpcServer<Identity, Stack<RpcRequestMetrics, Identity>>>,
}

// === impl RpcServer ===

impl RpcServer {
    fn empty() -> Self {
        Self { ws_http: Default::default(), ipc: None }
    }

    /// Returns the [`SocketAddr`] of the http server if started.
    pub const fn http_local_addr(&self) -> Option<SocketAddr> {
        self.ws_http.http_local_addr
    }
    /// Return the JwtSecret of the server
    pub const fn jwt(&self) -> Option<JwtSecret> {
        self.ws_http.jwt_secret
    }

    /// Returns the [`SocketAddr`] of the ws server if started.
    pub const fn ws_local_addr(&self) -> Option<SocketAddr> {
        self.ws_http.ws_local_addr
    }

    /// Returns the endpoint of the ipc server if started.
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc.as_ref().map(|ipc| ipc.endpoint())
    }

    /// Starts the configured server by spawning the servers on the tokio runtime.
    ///
    /// This returns an [RpcServerHandle] that's connected to the server task(s) until the server is
    /// stopped or the [RpcServerHandle] is dropped.
    #[instrument(name = "start", skip_all, fields(http = ?self.http_local_addr(), ws = ?self.ws_local_addr(), ipc = ?self.ipc_endpoint()), target = "rpc", level = "TRACE")]
    pub async fn start(self, modules: TransportRpcModules) -> Result<RpcServerHandle, RpcError> {
        trace!(target: "rpc", "staring RPC server");
        let Self { ws_http, ipc: ipc_server } = self;
        let TransportRpcModules { config, http, ws, ipc } = modules;
        let mut handle = RpcServerHandle {
            http_local_addr: ws_http.http_local_addr,
            ws_local_addr: ws_http.ws_local_addr,
            http: None,
            ws: None,
            ipc_endpoint: None,
            ipc: None,
            jwt_secret: None,
        };

        let (http, ws) = ws_http.server.start(http, ws, &config).await?;
        handle.http = http;
        handle.ws = ws;

        if let Some((server, module)) =
            ipc_server.and_then(|server| ipc.map(|module| (server, module)))
        {
            handle.ipc_endpoint = Some(server.endpoint());
            handle.ipc = Some(server.start(module).await?);
        }

        Ok(handle)
    }
}

impl fmt::Debug for RpcServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcServer")
            .field("http", &self.ws_http.http_local_addr.is_some())
            .field("ws", &self.ws_http.ws_local_addr.is_some())
            .field("ipc", &self.ipc.is_some())
            .finish()
    }
}
