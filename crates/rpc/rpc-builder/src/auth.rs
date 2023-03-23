pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{core::Error as RpcError, server::ServerHandle, RpcModule};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::ChainSpec;
use reth_provider::{BlockProvider, EvmEnvProvider, HeaderProvider, StateProviderFactory};
use reth_rpc::{
    eth::cache::EthStateCache, AuthLayer, EngineApi, EthApi, EthFilter, JwtAuthValidator, JwtSecret,
};
use reth_rpc_api::servers::*;
use reth_rpc_engine_api::EngineApiHandle;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{net::SocketAddr, sync::Arc};

/// Configure and launch an auth server with `engine` and a _new_ `eth` namespace.
#[allow(clippy::too_many_arguments)]
pub async fn launch<Client, Pool, Network, Tasks>(
    client: Client,
    pool: Pool,
    network: Network,
    executor: Tasks,
    chain_spec: Arc<ChainSpec>,
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
    let eth_api = EthApi::new(client.clone(), pool.clone(), network, eth_cache);
    let eth_filter = EthFilter::new(client, pool);
    launch_with_eth_api(eth_api, chain_spec, eth_filter, handle, socket_addr, secret).await
}

/// Configure and launch an auth server with existing EthApi implementation.
pub async fn launch_with_eth_api<Client, Pool, Network>(
    eth_api: EthApi<Client, Pool, Network>,
    chain_spec: Arc<ChainSpec>,
    eth_filter: EthFilter<Client, Pool>,
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
    module.merge(EngineApi::new(chain_spec, handle).into_rpc()).expect("No conflicting methods");
    module.merge(eth_api.into_rpc()).expect("No conflicting methods");
    module.merge(eth_filter.into_rpc()).expect("No conflicting methods");

    // Create auth middleware.
    let middleware =
        tower::ServiceBuilder::new().layer(AuthLayer::new(JwtAuthValidator::new(secret)));

    // By default, both http and ws are enabled.
    let server = ServerBuilder::new().set_middleware(middleware).build(socket_addr).await?;

    server.start(module)
}
