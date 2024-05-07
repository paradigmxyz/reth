//! Support for customizing the node

use crate::{
    args::{
        get_secret_key, DatabaseArgs, DebugArgs, DevArgs, DiscoveryArgs, NetworkArgs,
        PayloadBuilderArgs, PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    dirs::{ChainPath, DataDirPath},
    metrics::prometheus_exporter,
    utils::get_single_header,
};
use discv5::ListenConfig;
use metrics_exporter_prometheus::PrometheusHandle;
use once_cell::sync::Lazy;
use reth_config::{config::PruneConfig, Config};
use reth_db::{database::Database, database_metrics::DatabaseMetrics};
use reth_interfaces::{p2p::headers::client::HeadersClient, RethResult};
use reth_network::{NetworkBuilder, NetworkConfig, NetworkManager};
use reth_primitives::{
    constants::eip4844::MAINNET_KZG_TRUSTED_SETUP, kzg::KzgSettings, stage::StageId,
    BlockHashOrNumber, BlockNumber, ChainSpec, Head, SealedHeader, B256, MAINNET,
};
use reth_provider::{
    providers::StaticFileProvider, BlockHashReader, BlockNumReader, HeaderProvider,
    ProviderFactory, StageCheckpointReader,
};
use reth_tasks::TaskExecutor;
use secp256k1::SecretKey;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    path::PathBuf,
    sync::Arc,
};
use tracing::*;

/// The default prometheus recorder handle. We use a global static to ensure that it is only
/// installed once.
pub static PROMETHEUS_RECORDER_HANDLE: Lazy<PrometheusHandle> =
    Lazy::new(|| prometheus_exporter::install_recorder().unwrap());

/// This includes all necessary configuration to launch the node.
/// The individual configuration options can be overwritten before launching the node.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_builder::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///
///     // create the builder
///     let builder = NodeConfig::default();
///
///     // configure the rpc apis
///     let mut rpc = RpcServerArgs::default().with_http().with_ws();
///     rpc.http_api = Some(RpcModuleSelection::All);
///     let builder = builder.with_rpc(rpc);
/// }
/// ```
///
/// This can also be used to launch a node with a temporary test database. This can be done with
/// the [NodeConfig::test] method.
///
/// # Example
/// ```rust
/// # use reth_tasks::{TaskManager, TaskSpawner};
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_builder::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
///     let handle = Handle::current();
///     let manager = TaskManager::new(handle);
///     let executor = manager.executor();
///
///     // create the builder with a test database, using the `test` method
///     let builder = NodeConfig::test();
///
///     // configure the rpc apis
///     let mut rpc = RpcServerArgs::default().with_http().with_ws();
///     rpc.http_api = Some(RpcModuleSelection::All);
///     let builder = builder.with_rpc(rpc);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// The path to the configuration file to use.
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    pub chain: Arc<ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    pub metrics: Option<SocketAddr>,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - DISCOVERY_PORT: default + `instance` - 1
    /// - DISCOVERY_V5_PORT: default + `instance` - 1
    /// - AUTH_PORT: default + `instance` * 100 - 100
    /// - HTTP_RPC_PORT: default - `instance` + 1
    /// - WS_RPC_PORT: default + `instance` * 2 - 2
    pub instance: u16,

    /// All networking related arguments
    pub network: NetworkArgs,

    /// All rpc related arguments
    pub rpc: RpcServerArgs,

    /// All txpool related arguments with --txpool prefix
    pub txpool: TxPoolArgs,

    /// All payload builder related arguments
    pub builder: PayloadBuilderArgs,

    /// All debug related arguments with --debug prefix
    pub debug: DebugArgs,

    /// All database related arguments
    pub db: DatabaseArgs,

    /// All dev related arguments with --dev prefix
    pub dev: DevArgs,

    /// All pruning related arguments
    pub pruning: PruningArgs,
}

impl NodeConfig {
    /// Creates a testing [NodeConfig], causing the database to be launched ephemerally.
    pub fn test() -> Self {
        Self::default()
            // set all ports to zero by default for test instances
            .with_unused_ports()
    }

    /// Sets --dev mode for the node
    pub const fn dev(mut self) -> Self {
        self.dev.dev = true;
        self
    }

    /// Set the config file for the node
    pub fn with_config(mut self, config: impl Into<PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the [ChainSpec] for the node
    pub fn with_chain(mut self, chain: impl Into<Arc<ChainSpec>>) -> Self {
        self.chain = chain.into();
        self
    }

    /// Set the metrics address for the node
    pub fn with_metrics(mut self, metrics: SocketAddr) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the instance for the node
    pub fn with_instance(mut self, instance: u16) -> Self {
        self.instance = instance;
        self
    }

    /// Set the network args for the node
    pub fn with_network(mut self, network: NetworkArgs) -> Self {
        self.network = network;
        self
    }

    /// Set the rpc args for the node
    pub fn with_rpc(mut self, rpc: RpcServerArgs) -> Self {
        self.rpc = rpc;
        self
    }

    /// Set the txpool args for the node
    pub fn with_txpool(mut self, txpool: TxPoolArgs) -> Self {
        self.txpool = txpool;
        self
    }

    /// Set the builder args for the node
    pub fn with_payload_builder(mut self, builder: PayloadBuilderArgs) -> Self {
        self.builder = builder;
        self
    }

    /// Set the debug args for the node
    pub fn with_debug(mut self, debug: DebugArgs) -> Self {
        self.debug = debug;
        self
    }

    /// Set the database args for the node
    pub fn with_db(mut self, db: DatabaseArgs) -> Self {
        self.db = db;
        self
    }

    /// Set the dev args for the node
    pub fn with_dev(mut self, dev: DevArgs) -> Self {
        self.dev = dev;
        self
    }

    /// Set the pruning args for the node
    pub fn with_pruning(mut self, pruning: PruningArgs) -> Self {
        self.pruning = pruning;
        self
    }

    /// Get the network secret from the given data dir
    pub fn network_secret(&self, data_dir: &ChainPath<DataDirPath>) -> eyre::Result<SecretKey> {
        let network_secret_path =
            self.network.p2p_secret_key.clone().unwrap_or_else(|| data_dir.p2p_secret());
        debug!(target: "reth::cli", ?network_secret_path, "Loading p2p key file");
        let secret_key = get_secret_key(&network_secret_path)?;
        Ok(secret_key)
    }

    /// Returns the initial pipeline target, based on whether or not the node is running in
    /// `debug.tip` mode, `debug.continuous` mode, or neither.
    ///
    /// If running in `debug.tip` mode, the configured tip is returned.
    /// Otherwise, if running in `debug.continuous` mode, the genesis hash is returned.
    /// Otherwise, `None` is returned. This is what the node will do by default.
    pub fn initial_pipeline_target(&self, genesis_hash: B256) -> Option<B256> {
        if let Some(tip) = self.debug.tip {
            // Set the provided tip as the initial pipeline target.
            debug!(target: "reth::cli", %tip, "Tip manually set");
            Some(tip)
        } else if self.debug.continuous {
            // Set genesis as the initial pipeline target.
            // This will allow the downloader to start
            debug!(target: "reth::cli", "Continuous sync mode enabled");
            Some(genesis_hash)
        } else {
            None
        }
    }

    /// Returns pruning configuration.
    pub fn prune_config(&self) -> Option<PruneConfig> {
        self.pruning.prune_config(&self.chain)
    }

    /// Returns the max block that the node should run to, looking it up from the network if
    /// necessary
    pub async fn max_block<Provider, Client>(
        &self,
        network_client: Client,
        provider: Provider,
    ) -> eyre::Result<Option<BlockNumber>>
    where
        Provider: HeaderProvider,
        Client: HeadersClient,
    {
        let max_block = if let Some(block) = self.debug.max_block {
            Some(block)
        } else if let Some(tip) = self.debug.tip {
            Some(self.lookup_or_fetch_tip(provider, network_client, tip).await?)
        } else {
            None
        };

        Ok(max_block)
    }

    /// Create the [NetworkConfig] for the node
    pub fn network_config<C>(
        &self,
        config: &Config,
        client: C,
        executor: TaskExecutor,
        head: Head,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<NetworkConfig<C>> {
        info!(target: "reth::cli", "Connecting to P2P network");
        let secret_key = self.network_secret(data_dir)?;
        let default_peers_path = data_dir.known_peers();
        Ok(self.load_network_config(config, client, executor, head, secret_key, default_peers_path))
    }

    /// Create the [NetworkBuilder].
    ///
    /// This only configures it and does not spawn it.
    pub async fn build_network<C>(
        &self,
        config: &Config,
        client: C,
        executor: TaskExecutor,
        head: Head,
        data_dir: &ChainPath<DataDirPath>,
    ) -> eyre::Result<NetworkBuilder<C, (), ()>>
    where
        C: BlockNumReader,
    {
        let network_config = self.network_config(config, client, executor, head, data_dir)?;
        let builder = NetworkManager::builder(network_config).await?;
        Ok(builder)
    }

    /// Loads 'MAINNET_KZG_TRUSTED_SETUP'
    pub fn kzg_settings(&self) -> eyre::Result<Arc<KzgSettings>> {
        Ok(Arc::clone(&MAINNET_KZG_TRUSTED_SETUP))
    }

    /// Installs the prometheus recorder.
    pub fn install_prometheus_recorder(&self) -> eyre::Result<PrometheusHandle> {
        Ok(PROMETHEUS_RECORDER_HANDLE.clone())
    }

    /// Serves the prometheus endpoint over HTTP with the given database and prometheus handle.
    pub async fn start_metrics_endpoint<Metrics>(
        &self,
        prometheus_handle: PrometheusHandle,
        db: Metrics,
        static_file_provider: StaticFileProvider,
        task_executor: TaskExecutor,
    ) -> eyre::Result<()>
    where
        Metrics: DatabaseMetrics + 'static + Send + Sync,
    {
        if let Some(listen_addr) = self.metrics {
            info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");
            prometheus_exporter::serve(
                listen_addr,
                prometheus_handle,
                db,
                static_file_provider,
                metrics_process::Collector::default(),
                task_executor,
            )
            .await?;
        }

        Ok(())
    }

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    pub fn lookup_head<DB: Database>(&self, factory: ProviderFactory<DB>) -> RethResult<Head> {
        let provider = factory.provider()?;

        let head = provider.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;

        let header = provider
            .header_by_number(head)?
            .expect("the header for the latest block is missing, database is corrupt");

        let total_difficulty = provider
            .header_td_by_number(head)?
            .expect("the total difficulty for the latest block is missing, database is corrupt");

        let hash = provider
            .block_hash(head)?
            .expect("the hash for the latest block is missing, database is corrupt");

        Ok(Head {
            number: head,
            hash,
            difficulty: header.difficulty,
            total_difficulty,
            timestamp: header.timestamp,
        })
    }

    /// Attempt to look up the block number for the tip hash in the database.
    /// If it doesn't exist, download the header and return the block number.
    ///
    /// NOTE: The download is attempted with infinite retries.
    pub async fn lookup_or_fetch_tip<Provider, Client>(
        &self,
        provider: Provider,
        client: Client,
        tip: B256,
    ) -> RethResult<u64>
    where
        Provider: HeaderProvider,
        Client: HeadersClient,
    {
        let header = provider.header_by_hash_or_number(tip.into())?;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.number);
        }

        Ok(self.fetch_tip_from_network(client, tip.into()).await?.number)
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    pub async fn fetch_tip_from_network<Client>(
        &self,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> RethResult<SealedHeader>
    where
        Client: HeadersClient,
    {
        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        let mut fetch_failures = 0;
        loop {
            match get_single_header(&client, tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return Ok(tip_header);
                }
                Err(error) => {
                    fetch_failures += 1;
                    if fetch_failures % 20 == 0 {
                        error!(target: "reth::cli", ?fetch_failures, %error, "Failed to fetch the tip. Retrying...");
                    }
                }
            }
        }
    }

    /// Builds the [NetworkConfig] with the given [ProviderFactory].
    pub fn load_network_config<C>(
        &self,
        config: &Config,
        client: C,
        executor: TaskExecutor,
        head: Head,
        secret_key: SecretKey,
        default_peers_path: PathBuf,
    ) -> NetworkConfig<C> {
        let cfg_builder = self
            .network
            .network_config(config, self.chain.clone(), secret_key, default_peers_path)
            .with_task_executor(Box::new(executor))
            .set_head(head)
            .listener_addr(SocketAddr::new(
                self.network.addr,
                // set discovery port based on instance number
                self.network.port + self.instance - 1,
            ))
            .disable_discv4_discovery_if(self.chain.chain.is_optimism())
            .discovery_addr(SocketAddr::new(
                self.network.discovery.addr,
                // set discovery port based on instance number
                self.network.discovery.port + self.instance - 1,
            ));

        let config = cfg_builder.build(client);

        if self.network.discovery.disable_discovery ||
            !self.network.discovery.enable_discv5_discovery &&
                !config.chain_spec.chain.is_optimism()
        {
            return config
        }

        let rlpx_addr = config.listener_addr().ip();
        // work around since discv5 config builder can't be integrated into network config builder
        // due to unsatisfied trait bounds
        config.discovery_v5_with_config_builder(|builder| {
            let DiscoveryArgs {
                discv5_addr,
                discv5_addr_ipv6,
                discv5_port,
                discv5_port_ipv6,
                discv5_lookup_interval,
                discv5_bootstrap_lookup_interval,
                discv5_bootstrap_lookup_countdown,
                ..
            } = self.network.discovery;

            let discv5_addr_ipv4 = discv5_addr.or_else(|| ipv4(rlpx_addr));
            let discv5_addr_ipv6 = discv5_addr_ipv6.or_else(|| ipv6(rlpx_addr));
            let discv5_port_ipv4 = discv5_port + self.instance - 1;
            let discv5_port_ipv6 = discv5_port_ipv6 + self.instance - 1;

            builder
                .discv5_config(
                    discv5::ConfigBuilder::new(ListenConfig::from_two_sockets(
                        discv5_addr_ipv4.map(|addr| SocketAddrV4::new(addr, discv5_port_ipv4)),
                        discv5_addr_ipv6
                            .map(|addr| SocketAddrV6::new(addr, discv5_port_ipv6, 0, 0)),
                    ))
                    .build(),
                )
                .lookup_interval(discv5_lookup_interval)
                .bootstrap_lookup_interval(discv5_bootstrap_lookup_interval)
                .bootstrap_lookup_countdown(discv5_bootstrap_lookup_countdown)
                .build()
        })
    }

    /// Change rpc port numbers based on the instance number, using the inner
    /// [RpcServerArgs::adjust_instance_ports] method.
    pub fn adjust_instance_ports(&mut self) {
        self.rpc.adjust_instance_ports(self.instance);
    }

    /// Sets networking and RPC ports to zero, causing the OS to choose random unused ports when
    /// sockets are bound.
    pub fn with_unused_ports(mut self) -> Self {
        self.rpc = self.rpc.with_unused_ports();
        self.network = self.network.with_unused_ports();
        self
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            config: None,
            chain: MAINNET.clone(),
            metrics: None,
            instance: 1,
            network: NetworkArgs::default(),
            rpc: RpcServerArgs::default(),
            txpool: TxPoolArgs::default(),
            builder: PayloadBuilderArgs::default(),
            debug: DebugArgs::default(),
            db: DatabaseArgs::default(),
            dev: DevArgs::default(),
            pruning: PruningArgs::default(),
        }
    }
}

/// Returns the address if this is an [`Ipv4Addr`].
pub fn ipv4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

/// Returns the address if this is an [`Ipv6Addr`].
pub fn ipv6(ip: IpAddr) -> Option<Ipv6Addr> {
    match ip {
        IpAddr::V4(_) => None,
        IpAddr::V6(ip) => Some(ip),
    }
}
