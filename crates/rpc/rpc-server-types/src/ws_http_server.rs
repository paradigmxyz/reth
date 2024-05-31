use std::net::SocketAddr;

use jsonrpsee::{
    server::{Server, ServerHandle},
    RpcModule,
};
use reth_rpc_layer::{AuthLayer, JwtAuthValidator, JwtSecret};
use tower::layer::util::{Identity, Stack};
use tower_http::cors::CorsLayer;

use crate::{error::RpcError, metrics::RpcRequestMetrics};

/// Container type for ws and http servers in all possible combinations.
#[derive(Default)]
pub struct WsHttpServer {
    /// The address of the http server
    http_local_addr: Option<SocketAddr>,
    /// The address of the ws server
    ws_local_addr: Option<SocketAddr>,
    /// Configured ws,http servers
    server: WsHttpServers,
    /// The jwt secret.
    jwt_secret: Option<JwtSecret>,
}

// Define the type alias with detailed type complexity
type WsHttpServerKind = Server<
    Stack<
        tower::util::Either<AuthLayer<JwtAuthValidator>, Identity>,
        Stack<tower::util::Either<CorsLayer, Identity>, Identity>,
    >,
    Stack<RpcRequestMetrics, Identity>,
>;

/// Enum for holding the http and ws servers in all possible combinations.
enum WsHttpServers {
    /// Both servers are on the same port
    SamePort(WsHttpServerKind),
    /// Servers are on different ports
    DifferentPort { http: Option<WsHttpServerKind>, ws: Option<WsHttpServerKind> },
}

// === impl WsHttpServers ===

impl WsHttpServers {
    /// Starts the servers and returns the handles (http, ws)
    async fn start(
        self,
        http_module: Option<RpcModule<()>>,
        ws_module: Option<RpcModule<()>>,
        config: &TransportRpcModuleConfig,
    ) -> Result<(Option<ServerHandle>, Option<ServerHandle>), RpcError> {
        let mut http_handle = None;
        let mut ws_handle = None;
        match self {
            Self::SamePort(server) => {
                // Make sure http and ws modules are identical, since we currently can't run
                // different modules on same server
                config.ensure_ws_http_identical()?;

                if let Some(module) = http_module.or(ws_module) {
                    let handle = server.start(module);
                    http_handle = Some(handle.clone());
                    ws_handle = Some(handle);
                }
            }
            Self::DifferentPort { http, ws } => {
                if let Some((server, module)) =
                    http.and_then(|server| http_module.map(|module| (server, module)))
                {
                    http_handle = Some(server.start(module));
                }
                if let Some((server, module)) =
                    ws.and_then(|server| ws_module.map(|module| (server, module)))
                {
                    ws_handle = Some(server.start(module));
                }
            }
        }

        Ok((http_handle, ws_handle))
    }
}

impl Default for WsHttpServers {
    fn default() -> Self {
        Self::DifferentPort { http: None, ws: None }
    }
}
