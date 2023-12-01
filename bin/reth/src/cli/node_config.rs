//! Support for customizing the node
use crate::{
    args::{
        DatabaseArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs, PruningArgs,
        RpcServerArgs, TxPoolArgs,
    },
    cli::RethCliExt,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use reth_primitives::ChainSpec;

/// Start the node
#[derive(Debug)]
pub struct NodeConfig {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    pub datadir: PathBuf,

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
    /// - AUTH_PORT: default + `instance` * 100 - 100
    /// - HTTP_RPC_PORT: default - `instance` + 1
    /// - WS_RPC_PORT: default + `instance` * 2 - 2
    pub instance: u16,

    /// Overrides the KZG trusted setup by reading from the supplied file.
    pub trusted_setup_file: Option<PathBuf>,

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

    /// Rollup related arguments
    #[cfg(feature = "optimism")]
    pub rollup: crate::args::RollupArgs,
}

impl NodeConfig {
    /// Launches the node, also adding any RPC extensions passed.
    pub fn launch<E: RethCliExt>(self, ext: E::Node) -> NodeHandle {
        todo!()
    }

    /// Set the datadir for the node
    pub fn datadir(mut self, datadir: impl Into<PathBuf>) -> Self {
        self.datadir = datadir.into();
        self
    }

    /// Set the config file for the node
    pub fn config(mut self, config: impl Into<PathBuf>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the chain for the node
    pub fn chain(mut self, chain: Arc<ChainSpec>) -> Self {
        self.chain = chain.into();
        self
    }

    /// Set the metrics address for the node
    pub fn metrics(mut self, metrics: SocketAddr) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Set the instance for the node
    pub fn instance(mut self, instance: u16) -> Self {
        self.instance = instance;
        self
    }

    /// Set the trusted setup file for the node
    pub fn trusted_setup_file(mut self, trusted_setup_file: impl Into<PathBuf>) -> Self {
        self.trusted_setup_file = Some(trusted_setup_file.into());
        self
    }

    /// Set the network args for the node
    pub fn network(mut self, network: NetworkArgs) -> Self {
        self.network = network;
        self
    }

    /// Set the rpc args for the node
    pub fn rpc(mut self, rpc: RpcServerArgs) -> Self {
        self.rpc = rpc;
        self
    }

    /// Set the txpool args for the node
    pub fn txpool(mut self, txpool: TxPoolArgs) -> Self {
        self.txpool = txpool;
        self
    }

    /// Set the builder args for the node
    pub fn builder(mut self, builder: PayloadBuilderArgs) -> Self {
        self.builder = builder;
        self
    }

    /// Set the debug args for the node
    pub fn debug(mut self, debug: DebugArgs) -> Self {
        self.debug = debug;
        self
    }

    /// Set the database args for the node
    pub fn db(mut self, db: DatabaseArgs) -> Self {
        self.db = db;
        self
    }

    /// Set the dev args for the node
    pub fn dev(mut self, dev: DevArgs) -> Self {
        self.dev = dev;
        self
    }

    /// Set the pruning args for the node
    pub fn pruning(mut self, pruning: PruningArgs) -> Self {
        self.pruning = pruning;
        self
    }

    /// Set the rollup args for the node
    #[cfg(feature = "optimism")]
    pub fn rollup(mut self, rollup: crate::args::RollupArgs) -> Self {
        self.rollup = rollup;
        self
    }
}

/// The node handle
#[derive(Debug)]
pub struct NodeHandle;

#[cfg(test)]
mod tests {
    #[test]
    fn test_node_config() {
        use super::*;
        use crate::args::NetworkArgs;
        use reth_primitives::ChainSpec;
        use std::net::Ipv4Addr;

        let config = NodeConfig::default()
            .datadir("/tmp")
            .config("/tmp/config.toml")
            .chain(Arc::new(ChainSpec::default()))
            .metrics(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080))
            .instance(1)
            .trusted_setup_file("/tmp/trusted_setup")
            .network(NetworkArgs::default())
            .rpc(RpcServerArgs::default())
            .txpool(TxPoolArgs::default())
            .builder(PayloadBuilderArgs::default())
            .debug(DebugArgs::default())
            .db(DatabaseArgs::default())
            .dev(DevArgs::default())
            .pruning(PruningArgs::default());

        assert_eq!(config.datadir, PathBuf::from("/tmp"));
        assert_eq!(config.config, Some(PathBuf::from("/tmp/config.toml")));
        assert_eq!(config.chain, Arc::new(ChainSpec::default()));
        assert_eq!(config.metrics, Some(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 8080)));
        assert_eq!(config.instance, 1);
        assert_eq!(config.trusted_setup_file, Some(PathBuf::from("/tmp/trusted_setup")));
        assert_eq!(config.network, NetworkArgs::default());
        assert_eq!(config.rpc, RpcServerArgs::default());
        assert_eq!(config.txpool, TxPoolArgs::default());
        assert_eq!(config.builder, PayloadBuilderArgs::default());
        assert_eq!(config.debug, DebugArgs::default());
        assert_eq!(config.db, DatabaseArgs::default());
        assert_eq!(config.dev, DevArgs::default());
        assert_eq!(config.pruning, PruningArgs::default());
    }
}
