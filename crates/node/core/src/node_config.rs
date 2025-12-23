//! Support for customizing the node

use crate::{
    args::{
        DatabaseArgs, DatadirArgs, DebugArgs, DevArgs, EngineArgs, NetworkArgs, PayloadBuilderArgs,
        PruningArgs, RpcServerArgs, StaticFilesArgs, TxPoolArgs,
    },
    dirs::{ChainPath, DataDirPath},
    utils::get_single_header,
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockNumber, B256, U256};
use eyre::eyre;
use reth_chainspec::{ChainSpec, EthChainSpec, MAINNET};
use reth_config::config::PruneConfig;
use reth_engine_local::MiningMode;
use reth_ethereum_forks::{EthereumHardforks, Head};
use reth_network_p2p::headers::client::HeadersClient;
use reth_primitives_traits::SealedHeader;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockHashReader, DatabaseProviderFactory, HeaderProvider, StageCheckpointReader,
};
use reth_storage_errors::provider::ProviderResult;
use reth_transaction_pool::TransactionPool;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::*;

use crate::args::{EraArgs, MetricArgs};
pub use reth_engine_primitives::{
    DEFAULT_MEMORY_BLOCK_BUFFER_TARGET, DEFAULT_PERSISTENCE_THRESHOLD, DEFAULT_RESERVED_CPU_CORES,
};

/// Default size of cross-block cache in megabytes.
pub const DEFAULT_CROSS_BLOCK_CACHE_SIZE_MB: usize = 4 * 1024;

/// This includes all necessary configuration to launch the node.
/// The individual configuration options can be overwritten before launching the node.
///
/// # Example
/// ```rust
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_server_types::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
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
/// the [`NodeConfig::test`] method.
///
/// # Example
/// ```rust
/// # use reth_node_core::{
/// #     node_config::NodeConfig,
/// #     args::RpcServerArgs,
/// # };
/// # use reth_rpc_server_types::RpcModuleSelection;
/// # use tokio::runtime::Handle;
///
/// async fn t() {
///     // create the builder with a test database, using the `test` method
///     let builder = NodeConfig::test();
///
///     // configure the rpc apis
///     let mut rpc = RpcServerArgs::default().with_http().with_ws();
///     rpc.http_api = Some(RpcModuleSelection::All);
///     let builder = builder.with_rpc(rpc);
/// }
/// ```
#[derive(Debug)]
pub struct NodeConfig<ChainSpec> {
    /// All data directory related arguments
    pub datadir: DatadirArgs,

    /// The path to the configuration file to use.
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    pub chain: Arc<ChainSpec>,

    /// Enable to configure metrics export to endpoints
    pub metrics: MetricArgs,

    /// Add a new instance of a node.
    ///
    /// Configures the ports of the node to avoid conflicts with the defaults.
    /// This is useful for running multiple nodes on the same machine.
    ///
    /// Max number of instances is 200. It is chosen in a way so that it's not possible to have
    /// port numbers that conflict with each other.
    ///
    /// Changes to the following port numbers:
    /// - `DISCOVERY_PORT`: default + `instance` - 1
    /// - `DISCOVERY_V5_PORT`: default + `instance` - 1
    /// - `AUTH_PORT`: default + `instance` * 100 - 100
    /// - `HTTP_RPC_PORT`: default - `instance` + 1
    /// - `WS_RPC_PORT`: default + `instance` * 2 - 2
    /// - `IPC_PATH`: default + `instance`
    pub instance: Option<u16>,

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

    /// All engine related arguments
    pub engine: EngineArgs,

    /// All ERA import related arguments with --era prefix
    pub era: EraArgs,

    /// All static files related arguments
    pub static_files: StaticFilesArgs,
}

impl NodeConfig<ChainSpec> {
    /// Creates a testing [`NodeConfig`], causing the database to be launched ephemerally.
    pub fn test() -> Self {
        Self::default()
            // set all ports to zero by default for test instances
            .with_unused_ports()
    }
}

impl<ChainSpec> NodeConfig<ChainSpec> {
    /// Creates a new config with given chain spec, setting all fields to default values.
    pub fn new(chain: Arc<ChainSpec>) -> Self {
        Self {
            config: None,
            chain,
            metrics: MetricArgs::default(),
            instance: None,
            network: NetworkArgs::default(),
            rpc: RpcServerArgs::default(),
            txpool: TxPoolArgs::default(),
            builder: PayloadBuilderArgs::default(),
            debug: DebugArgs::default(),
            db: DatabaseArgs::default(),
            dev: DevArgs::default(),
            pruning: PruningArgs::default(),
            datadir: DatadirArgs::default(),
            engine: EngineArgs::default(),
            era: EraArgs::default(),
            static_files: StaticFilesArgs::default(),
        }
    }

    /// Sets --dev mode for the node.
    ///
    /// In addition to setting the `--dev` flag, this also:
    ///   - disables discovery in [`NetworkArgs`].
    pub const fn dev(mut self) -> Self {
        self.dev.dev = true;
        self.network.discovery.disable_discovery = true;
        self
    }

    /// Apply a function to the config.
    pub fn apply<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    /// Applies a fallible function to the config.
    pub fn try_apply<F, R>(self, f: F) -> Result<Self, R>
    where
        F: FnOnce(Self) -> Result<Self, R>,
    {
        f(self)
    }

    /// Sets --dev mode for the node [`NodeConfig::dev`], if `dev` is true.
    pub const fn set_dev(self, dev: bool) -> Self {
        if dev {
            self.dev()
        } else {
            self
        }
    }

    /// Set the data directory args for the node
    pub fn with_datadir_args(mut self, datadir_args: DatadirArgs) -> Self {
        self.datadir = datadir_args;
        self
    }

    /// Set the config file for the node
    pub fn with_config(mut self, config: impl Into<PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the [`ChainSpec`] for the node
    pub fn with_chain(mut self, chain: impl Into<Arc<ChainSpec>>) -> Self {
        self.chain = chain.into();
        self
    }

    /// Set the [`ChainSpec`] for the node and converts the type to that chainid.
    pub fn map_chain<C>(self, chain: impl Into<Arc<C>>) -> NodeConfig<C> {
        let Self {
            datadir,
            config,
            metrics,
            instance,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            engine,
            era,
            static_files,
            ..
        } = self;
        NodeConfig {
            datadir,
            config,
            chain: chain.into(),
            metrics,
            instance,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            engine,
            era,
            static_files,
        }
    }

    /// Set the metrics address for the node
    pub fn with_metrics(mut self, metrics: MetricArgs) -> Self {
        self.metrics = metrics;
        self
    }

    /// Set the instance for the node
    pub const fn with_instance(mut self, instance: u16) -> Self {
        self.instance = Some(instance);
        self
    }

    /// Returns the instance value, defaulting to 1 if not set.
    pub fn get_instance(&self) -> u16 {
        self.instance.unwrap_or(1)
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
    pub const fn with_db(mut self, db: DatabaseArgs) -> Self {
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

    /// Returns pruning configuration.
    pub fn prune_config(&self) -> Option<PruneConfig>
    where
        ChainSpec: EthereumHardforks,
    {
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
        Client: HeadersClient<Header: reth_primitives_traits::BlockHeader>,
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

    /// Fetches the head block from the database.
    ///
    /// If the database is empty, returns the genesis block.
    pub fn lookup_head<Factory>(&self, factory: &Factory) -> ProviderResult<Head>
    where
        Factory: DatabaseProviderFactory<
            Provider: HeaderProvider + StageCheckpointReader + BlockHashReader,
        >,
    {
        let provider = factory.database_provider_ro()?;

        let head = provider.get_stage_checkpoint(StageId::Finish)?.unwrap_or_default().block_number;

        let header = provider
            .header_by_number(head)?
            .expect("the header for the latest block is missing, database is corrupt");

        let hash = provider
            .block_hash(head)?
            .expect("the hash for the latest block is missing, database is corrupt");

        Ok(Head {
            number: head,
            hash,
            difficulty: header.difficulty(),
            total_difficulty: U256::ZERO,
            timestamp: header.timestamp(),
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
    ) -> ProviderResult<u64>
    where
        Provider: HeaderProvider,
        Client: HeadersClient<Header: reth_primitives_traits::BlockHeader>,
    {
        let header = provider.header_by_hash_or_number(tip.into())?;

        // try to look up the header in the database
        if let Some(header) = header {
            info!(target: "reth::cli", ?tip, "Successfully looked up tip block in the database");
            return Ok(header.number())
        }

        Ok(self.fetch_tip_from_network(client, tip.into()).await.number())
    }

    /// Attempt to look up the block with the given number and return the header.
    ///
    /// NOTE: The download is attempted with infinite retries.
    pub async fn fetch_tip_from_network<Client>(
        &self,
        client: Client,
        tip: BlockHashOrNumber,
    ) -> SealedHeader<Client::Header>
    where
        Client: HeadersClient<Header: reth_primitives_traits::BlockHeader>,
    {
        info!(target: "reth::cli", ?tip, "Fetching tip block from the network.");
        let mut fetch_failures = 0;
        loop {
            match get_single_header(&client, tip).await {
                Ok(tip_header) => {
                    info!(target: "reth::cli", ?tip, "Successfully fetched tip");
                    return tip_header
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

    /// Change rpc port numbers based on the instance number, using the inner
    /// [`RpcServerArgs::adjust_instance_ports`] method.
    pub fn adjust_instance_ports(&mut self) {
        self.network.adjust_instance_ports(self.instance);
        self.rpc.adjust_instance_ports(self.instance);
    }

    /// Sets networking and RPC ports to zero, causing the OS to choose random unused ports when
    /// sockets are bound.
    pub fn with_unused_ports(mut self) -> Self {
        self.rpc = self.rpc.with_unused_ports();
        self.network = self.network.with_unused_ports();
        self
    }

    /// Disables all discovery services for the node.
    pub const fn with_disabled_discovery(mut self) -> Self {
        self.network.discovery.disable_discovery = true;
        self
    }

    /// Effectively disables the RPC state cache by setting the cache sizes to `0`.
    ///
    /// By setting the cache sizes to 0, caching of newly executed or fetched blocks will be
    /// effectively disabled.
    pub const fn with_disabled_rpc_cache(mut self) -> Self {
        self.rpc.rpc_state_cache.set_zero_lengths();
        self
    }

    /// Resolve the final datadir path.
    pub fn datadir(&self) -> ChainPath<DataDirPath>
    where
        ChainSpec: EthChainSpec,
    {
        self.datadir.clone().resolve_datadir(self.chain.chain())
    }

    /// Load an application configuration from a specified path.
    ///
    /// A new configuration file is created with default values if none
    /// exists.
    pub fn load_path<T: Serialize + DeserializeOwned + Default>(
        path: impl AsRef<Path>,
    ) -> eyre::Result<T> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(cfg_string) => {
                toml::from_str(&cfg_string).map_err(|e| eyre!("Failed to parse TOML: {e}"))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| eyre!("Failed to create directory: {e}"))?;
                }
                let cfg = T::default();
                let s = toml::to_string_pretty(&cfg)
                    .map_err(|e| eyre!("Failed to serialize to TOML: {e}"))?;
                fs::write(path, s).map_err(|e| eyre!("Failed to write configuration file: {e}"))?;
                Ok(cfg)
            }
            Err(e) => Err(eyre!("Failed to load configuration: {e}")),
        }
    }

    /// Modifies the [`ChainSpec`] generic of the config using the provided closure.
    pub fn map_chainspec<F, C>(self, f: F) -> NodeConfig<C>
    where
        F: FnOnce(Arc<ChainSpec>) -> C,
    {
        let chain = Arc::new(f(self.chain));
        NodeConfig {
            chain,
            datadir: self.datadir,
            config: self.config,
            metrics: self.metrics,
            instance: self.instance,
            network: self.network,
            rpc: self.rpc,
            txpool: self.txpool,
            builder: self.builder,
            debug: self.debug,
            db: self.db,
            dev: self.dev,
            pruning: self.pruning,
            engine: self.engine,
            era: self.era,
            static_files: self.static_files,
        }
    }

    /// Returns the [`MiningMode`] intended for --dev mode.
    pub fn dev_mining_mode<Pool>(&self, pool: Pool) -> MiningMode<Pool>
    where
        Pool: TransactionPool + Unpin,
    {
        if let Some(interval) = self.dev.block_time {
            MiningMode::interval(interval)
        } else {
            MiningMode::instant(pool, self.dev.block_max_transactions)
        }
    }
}

impl Default for NodeConfig<ChainSpec> {
    fn default() -> Self {
        Self::new(MAINNET.clone())
    }
}

impl<ChainSpec> Clone for NodeConfig<ChainSpec> {
    fn clone(&self) -> Self {
        Self {
            chain: self.chain.clone(),
            config: self.config.clone(),
            metrics: self.metrics.clone(),
            instance: self.instance,
            network: self.network.clone(),
            rpc: self.rpc.clone(),
            txpool: self.txpool.clone(),
            builder: self.builder.clone(),
            debug: self.debug.clone(),
            db: self.db,
            dev: self.dev.clone(),
            pruning: self.pruning.clone(),
            datadir: self.datadir.clone(),
            engine: self.engine.clone(),
            era: self.era.clone(),
            static_files: self.static_files,
        }
    }
}
