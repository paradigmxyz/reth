//! Main node command for launching a node

use clap::{value_parser, Args, Parser};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_cli_util::parse_socket_address;
use reth_db::{init_db, DatabaseEnv};
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use reth_node_core::{
    args::{
        DatabaseArgs, DatadirArgs, DebugArgs, DevArgs, NetworkArgs, PayloadBuilderArgs,
        PruningArgs, RpcServerArgs, TxPoolArgs,
    },
    node_config::NodeConfig,
    version,
};
use reth_node_metrics::recorder::install_prometheus_recorder;
use std::{ffi::OsString, fmt, future::Future, net::SocketAddr, path::PathBuf, sync::Arc};

/// Start the node
#[derive(Debug, Parser)]
pub struct NodeCommand<
    C: ChainSpecParser = EthereumChainSpecParser,
    Ext: clap::Args + fmt::Debug = NoArgs,
> {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    pub config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = C::help_message(),
        default_value = C::SUPPORTED_CHAINS[0],
        default_value_if("dev", "true", "dev"),
        value_parser = C::parser(),
        required = false,
    )]
    pub chain: Arc<C::ChainSpec>,

    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
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
    /// - `DISCOVERY_PORT`: default + `instance` - 1
    /// - `AUTH_PORT`: default + `instance` * 100 - 100
    /// - `HTTP_RPC_PORT`: default - `instance` + 1
    /// - `WS_RPC_PORT`: default + `instance` * 2 - 2
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    pub instance: u16,

    /// Sets all ports to unused, allowing the OS to choose random unused ports when sockets are
    /// bound.
    ///
    /// Mutually exclusive with `--instance`.
    #[arg(long, conflicts_with = "instance", global = true)]
    pub with_unused_ports: bool,

    /// All datadir related arguments
    #[command(flatten)]
    pub datadir: DatadirArgs,

    /// All networking related arguments
    #[command(flatten)]
    pub network: NetworkArgs,

    /// All rpc related arguments
    #[command(flatten)]
    pub rpc: RpcServerArgs,

    /// All txpool related arguments with --txpool prefix
    #[command(flatten)]
    pub txpool: TxPoolArgs,

    /// All payload builder related arguments
    #[command(flatten)]
    pub builder: PayloadBuilderArgs,

    /// All debug related arguments with --debug prefix
    #[command(flatten)]
    pub debug: DebugArgs,

    /// All database related arguments
    #[command(flatten)]
    pub db: DatabaseArgs,

    /// All dev related arguments with --dev prefix
    #[command(flatten)]
    pub dev: DevArgs,

    /// All pruning related arguments
    #[command(flatten)]
    pub pruning: PruningArgs,

    /// Additional cli arguments
    #[command(flatten, next_help_heading = "Extension")]
    pub ext: Ext,
}

impl<C: ChainSpecParser> NodeCommand<C> {
    /// Parsers only the default CLI arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Parsers only the default [`NodeCommand`] arguments from the given iterator
    pub fn try_parse_args_from<I, T>(itr: I) -> Result<Self, clap::error::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        Self::try_parse_from(itr)
    }
}

impl<
        C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>,
        Ext: clap::Args + fmt::Debug,
    > NodeCommand<C, Ext>
{
    /// Launches the node
    ///
    /// This transforms the node command into a node config and launches the node using the given
    /// closure.
    pub async fn execute<L, Fut>(self, ctx: CliContext, launcher: L) -> eyre::Result<()>
    where
        L: FnOnce(WithLaunchContext<NodeBuilder<Arc<DatabaseEnv>, C::ChainSpec>>, Ext) -> Fut,
        Fut: Future<Output = eyre::Result<()>>,
    {
        tracing::info!(target: "reth::cli", version = ?version::SHORT_VERSION, "Starting reth");

        let Self {
            datadir,
            config,
            chain,
            metrics,
            instance,
            with_unused_ports,
            network,
            rpc,
            txpool,
            builder,
            debug,
            db,
            dev,
            pruning,
            ext,
        } = self;

        // set up node config
        let mut node_config = NodeConfig {
            datadir,
            config,
            chain,
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
        };

        // Register the prometheus recorder before creating the database,
        // because database init needs it to register metrics.
        let _ = install_prometheus_recorder();

        let data_dir = node_config.datadir();
        let db_path = data_dir.db();

        tracing::info!(target: "reth::cli", path = ?db_path, "Opening database");
        let database = Arc::new(init_db(db_path.clone(), self.db.database_args())?.with_metrics());

        if with_unused_ports {
            node_config = node_config.with_unused_ports();
        }

        let builder = NodeBuilder::new(node_config)
            .with_database(database)
            .with_launch_context(ctx.task_executor);

        launcher(builder, ext).await
    }
}

/// No Additional arguments
#[derive(Debug, Clone, Copy, Default, Args)]
#[non_exhaustive]
pub struct NoArgs;

#[cfg(test)]
mod tests {
    use super::*;
    use reth_discv4::DEFAULT_DISCOVERY_PORT;
    use reth_ethereum_cli::chainspec::SUPPORTED_CHAINS;
    use std::{
        net::{IpAddr, Ipv4Addr},
        path::Path,
    };

    #[test]
    fn parse_help_node_command() {
        let err = NodeCommand::<EthereumChainSpecParser>::try_parse_args_from(["reth", "--help"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
    }

    #[test]
    fn parse_common_node_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: NodeCommand = NodeCommand::parse_from(["reth", "--chain", chain]);
            assert_eq!(args.chain.chain, chain.parse::<reth_chainspec::Chain>().unwrap());
        }
    }

    #[test]
    fn parse_discovery_addr() {
        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--discovery.addr", "127.0.0.1"]).unwrap();
        assert_eq!(cmd.network.discovery.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_addr() {
        let cmd: NodeCommand = NodeCommand::try_parse_args_from([
            "reth",
            "--discovery.addr",
            "127.0.0.1",
            "--addr",
            "127.0.0.1",
        ])
        .unwrap();
        assert_eq!(cmd.network.discovery.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(cmd.network.addr, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn parse_discovery_port() {
        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--discovery.port", "300"]).unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
    }

    #[test]
    fn parse_port() {
        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--discovery.port", "300", "--port", "99"])
                .unwrap();
        assert_eq!(cmd.network.discovery.port, 300);
        assert_eq!(cmd.network.port, 99);
    }

    #[test]
    fn parse_metrics_port() {
        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--metrics", "9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--metrics", ":9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));

        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--metrics", "localhost:9001"]).unwrap();
        assert_eq!(cmd.metrics, Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9001)));
    }

    #[test]
    fn parse_config_path() {
        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--config", "my/path/to/reth.toml"]).unwrap();
        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.resolve_datadir(cmd.chain.chain);
        let config_path = cmd.config.unwrap_or_else(|| data_dir.config());
        assert_eq!(config_path, Path::new("my/path/to/reth.toml"));

        let cmd: NodeCommand = NodeCommand::try_parse_args_from(["reth"]).unwrap();

        // always store reth.toml in the data dir, not the chain specific data dir
        let data_dir = cmd.datadir.resolve_datadir(cmd.chain.chain);
        let config_path = cmd.config.clone().unwrap_or_else(|| data_dir.config());
        let end = format!("{}/reth.toml", SUPPORTED_CHAINS[0]);
        assert!(config_path.ends_with(end), "{:?}", cmd.config);
    }

    #[test]
    fn parse_db_path() {
        let cmd: NodeCommand = NodeCommand::try_parse_args_from(["reth"]).unwrap();
        let data_dir = cmd.datadir.resolve_datadir(cmd.chain.chain);

        let db_path = data_dir.db();
        let end = format!("reth/{}/db", SUPPORTED_CHAINS[0]);
        assert!(db_path.ends_with(end), "{:?}", cmd.config);

        let cmd: NodeCommand =
            NodeCommand::try_parse_args_from(["reth", "--datadir", "my/custom/path"]).unwrap();
        let data_dir = cmd.datadir.resolve_datadir(cmd.chain.chain);

        let db_path = data_dir.db();
        assert_eq!(db_path, Path::new("my/custom/path/db"));
    }

    #[test]
    fn parse_instance() {
        let mut cmd: NodeCommand = NodeCommand::parse_from(["reth"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8551);
        assert_eq!(cmd.rpc.http_port, 8545);
        assert_eq!(cmd.rpc.ws_port, 8546);
        // check network listening port number
        assert_eq!(cmd.network.port, 30303);

        let mut cmd: NodeCommand = NodeCommand::parse_from(["reth", "--instance", "2"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8651);
        assert_eq!(cmd.rpc.http_port, 8544);
        assert_eq!(cmd.rpc.ws_port, 8548);
        // check network listening port number
        assert_eq!(cmd.network.port, 30304);

        let mut cmd: NodeCommand = NodeCommand::parse_from(["reth", "--instance", "3"]);
        cmd.rpc.adjust_instance_ports(cmd.instance);
        cmd.network.port = DEFAULT_DISCOVERY_PORT + cmd.instance - 1;
        // check rpc port numbers
        assert_eq!(cmd.rpc.auth_port, 8751);
        assert_eq!(cmd.rpc.http_port, 8543);
        assert_eq!(cmd.rpc.ws_port, 8550);
        // check network listening port number
        assert_eq!(cmd.network.port, 30305);
    }

    #[test]
    fn parse_with_unused_ports() {
        let cmd: NodeCommand = NodeCommand::parse_from(["reth", "--with-unused-ports"]);
        assert!(cmd.with_unused_ports);
    }

    #[test]
    fn with_unused_ports_conflicts_with_instance() {
        let err = NodeCommand::<EthereumChainSpecParser>::try_parse_args_from([
            "reth",
            "--with-unused-ports",
            "--instance",
            "2",
        ])
        .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn with_unused_ports_check_zero() {
        let mut cmd: NodeCommand = NodeCommand::parse_from(["reth"]);
        cmd.rpc = cmd.rpc.with_unused_ports();
        cmd.network = cmd.network.with_unused_ports();

        // make sure the rpc ports are zero
        assert_eq!(cmd.rpc.auth_port, 0);
        assert_eq!(cmd.rpc.http_port, 0);
        assert_eq!(cmd.rpc.ws_port, 0);

        // make sure the network ports are zero
        assert_eq!(cmd.network.port, 0);
        assert_eq!(cmd.network.discovery.port, 0);

        // make sure the ipc path is not the default
        assert_ne!(cmd.rpc.ipcpath, String::from("/tmp/reth.ipc"));
    }
}
