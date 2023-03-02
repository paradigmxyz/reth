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
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc::{
    eth::cache::EthStateCache, AdminApi, AuthLayer, DebugApi, EngineApi, EthApi, JwtAuthValidator,
    JwtSecret, NetApi, TraceApi, Web3Api,
};
use reth_rpc_api::servers::*;
use reth_rpc_engine_api::EngineApiHandle;
use reth_tasks::TaskSpawner;
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

/// Configure and launch an auth server with `engine` and a _new_ `eth` namespace.
pub async fn launch<Client, Pool, Network, Tasks>(
    client: Client,
    pool: Pool,
    network: Network,
    executor: Tasks,
    handle: EngineApiHandle,
    socket_addr: SocketAddr,
    secret: JwtSecret,
) -> Result<ServerHandle, RpcError>
where
    Client: BlockProvider
        + HeaderProvider
        + StateProviderFactory
        + EvmEnvProvider
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
{
    // spawn a new cache task
    let eth_cache = EthStateCache::spawn_with(client.clone(), Default::default(), executor);
    launch_with_eth_api(EthApi::new(client, pool, network, eth_cache), handle, socket_addr, secret)
        .await
}

/// Configure and launch an auth server with existing EthApi implementation.
pub async fn launch_with_eth_api<Client, Pool, Network>(
    eth_api: EthApi<Client, Pool, Network>,
    handle: EngineApiHandle,
    socket_addr: SocketAddr,
    secret: JwtSecret,
) -> Result<ServerHandle, RpcError>
where
    Client: BlockProvider
        + HeaderProvider
        + StateProviderFactory
        + EvmEnvProvider
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
{
    // Configure the module and start the server.
    let mut module = RpcModule::new(());
    module.merge(EngineApi::new(handle).into_rpc());
    module.merge(eth_api.into_rpc());

    // Create auth middleware.
    let middleware =
        tower::ServiceBuilder::new().layer(AuthLayer::new(JwtAuthValidator::new(secret)));

    // By default, both http and ws are enabled.
    let server = ServerBuilder::new().set_middleware(middleware).build(socket_addr).await?;

    server.start(module)
}
