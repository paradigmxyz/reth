use crate::{constants::DEFAULT_AUTH_PORT, RpcServerConfig};
use hyper::{http::HeaderValue, Method};
pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{
    core::{
        server::{host_filtering::AllowHosts, rpc_module::Methods},
        Error as RpcError,
    },
    server::{middleware, Server, ServerHandle},
    RpcModule,
};
use reth_ipc::server::IpcServer;
pub use reth_ipc::server::{Builder as IpcServerBuilder, Endpoint};
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{BlockProvider, HeaderProvider, StateProviderFactory};
use reth_rpc::{
    AdminApi, AuthLayer, DebugApi, EngineApi, EthApi, JwtAuthValidator, JwtSecret, NetApi,
    TraceApi, Web3Api,
};
use reth_rpc_api::servers::*;
use reth_rpc_engine_api::EngineApiHandle;
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize, Serializer};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
};
use strum::{AsRefStr, EnumString, EnumVariantNames, ParseError, VariantNames};
use tower::layer::util::{Identity, Stack};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// TODO:
pub async fn launch(
    handle: EngineApiHandle,
    socket_addr: SocketAddr,
    secret: JwtSecret,
) -> Result<ServerHandle, RpcError> {
    // TODO: cors?
    let middleware =
        tower::ServiceBuilder::new().layer(AuthLayer::new(JwtAuthValidator::new(secret)));
    // By default both http and ws are enabled.
    let server = ServerBuilder::new().set_middleware(middleware).build(socket_addr).await?;
    let handle = server.start(EngineApi::new(handle).into_rpc())?;
    Ok(handle)
}
