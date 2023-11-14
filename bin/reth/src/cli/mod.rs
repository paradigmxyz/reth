//! CLI definition and entrypoint to executable
use crate::{
    args::utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
    chain,
    cli::ext::RethCliExt,
    db, debug_cmd,
    dirs::{LogsDir, PlatformPath},
    node, p2p, recover,
    runner::CliRunner,
    stage, test_vectors,
    version::{LONG_VERSION, SHORT_VERSION},
};
use clap::{value_parser, ArgAction, Args, Parser, Subcommand, ValueEnum};
use reth_primitives::ChainSpec;
use reth_tracing::{
    tracing::{metadata::LevelFilter, Level, Subscriber},
    tracing_subscriber::{filter::Directive, registry::LookupSpan, EnvFilter},
    BoxedLayer, FileWorkerGuard,
};
use std::{fmt, fmt::Display, sync::Arc};

pub mod components;
pub mod config;
pub mod ext;

/// Default [Directive] for [EnvFilter] which disables high-frequency debug logs from `hyper` and
/// `trust-dns`
const DEFAULT_ENV_FILTER_DIRECTIVE: &str =
    "hyper::proto::h1=off,trust_dns_proto=off,trust_dns_resolver=off";

/// The main reth cli interface.
///
/// This is the entrypoint to the executable.
#[derive(Debug, Parser)]
#[command(author, version = SHORT_VERSION, long_version = LONG_VERSION, about = "Reth", long_about = None)]
pub struct Cli<Ext: RethCliExt = ()> {
    /// The command to run
    #[clap(subcommand)]
    command: Commands<Ext>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

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
    #[arg(long, value_name = "INSTANCE", global = true, default_value_t = 1, value_parser = value_parser!(u16).range(..=200))]
    instance: u16,

    #[clap(flatten)]
    logs: Logs,

    #[clap(flatten)]
    verbosity: Verbosity,
}

impl<Ext: RethCliExt> Cli<Ext> {
    /// Execute the configured cli command.
    pub fn run(mut self) -> eyre::Result<()> {
        // add network name to logs dir
        self.logs.log_file_directory =
            self.logs.log_file_directory.join(self.chain.chain.to_string());

        let _guard = self.init_tracing()?;

        let runner = CliRunner;
        match self.command {
            Commands::Node(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
            Commands::Init(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Import(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Db(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::Stage(command) => runner.run_blocking_until_ctrl_c(command.execute()),
            Commands::P2P(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
            Commands::Debug(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
            Commands::Recover(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
        }
    }

    /// Initializes tracing with the configured options.
    ///
    /// If file logging is enabled, this function returns a guard that must be kept alive to ensure
    /// that all logs are flushed to disk.
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
        let mut layers =
            vec![reth_tracing::stdout(self.verbosity.directive(), &self.logs.color.to_string())];

        let (additional_layers, guard) = self.logs.layers()?;
        layers.extend(additional_layers);

        reth_tracing::init(layers);
        Ok(guard)
    }

    /// Configures the given node extension.
    pub fn with_node_extension<C>(mut self, conf: C) -> Self
    where
        C: Into<Ext::Node>,
    {
        self.command.set_node_extension(conf.into());
        self
    }
}

/// Convenience function for parsing CLI options, set up logging and run the chosen command.
#[inline]
pub fn run() -> eyre::Result<()> {
    Cli::<()>::parse().run()
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands<Ext: RethCliExt = ()> {
    /// Start the node
    #[command(name = "node")]
    Node(node::NodeCommand<Ext>),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(chain::InitCommand),
    /// This syncs RLP encoded blocks from a file.
    #[command(name = "import")]
    Import(chain::ImportCommand),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command),
    /// Manipulate individual stages.
    #[command(name = "stage")]
    Stage(stage::Command),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command),
    /// Generate Test Vectors
    #[command(name = "test-vectors")]
    TestVectors(test_vectors::Command),
    /// Write config to stdout
    #[command(name = "config")]
    Config(crate::config::Command),
    /// Various debug routines
    #[command(name = "debug")]
    Debug(debug_cmd::Command),
    /// Scripts for node recovery
    #[command(name = "recover")]
    Recover(recover::Command),
}

impl<Ext: RethCliExt> Commands<Ext> {
    /// Sets the node extension if it is the [NodeCommand](node::NodeCommand).
    ///
    /// This is a noop if the command is not the [NodeCommand](node::NodeCommand).
    pub fn set_node_extension(&mut self, ext: Ext::Node) {
        if let Commands::Node(command) = self {
            command.ext = ext
        }
    }
}

/// The log configuration.
#[derive(Debug, Args)]
#[command(next_help_heading = "Logging")]
pub struct Logs {
    /// The path to put log files in.
    #[arg(long = "log.file.directory", value_name = "PATH", global = true, default_value_t)]
    log_file_directory: PlatformPath<LogsDir>,

    /// The maximum size (in MB) of one log file.
    #[arg(long = "log.file.max-size", value_name = "SIZE", global = true, default_value_t = 200)]
    log_file_max_size: u64,

    /// The maximum amount of log files that will be stored. If set to 0, background file logging
    /// is disabled.
    #[arg(long = "log.file.max-files", value_name = "COUNT", global = true, default_value_t = 5)]
    log_file_max_files: usize,

    /// The filter to use for logs written to the log file.
    #[arg(long = "log.file.filter", value_name = "FILTER", global = true, default_value = "debug")]
    log_file_filter: String,

    /// Write logs to journald.
    #[arg(long = "log.journald", global = true)]
    journald: bool,

    /// The filter to use for logs written to journald.
    #[arg(
        long = "log.journald.filter",
        value_name = "FILTER",
        global = true,
        default_value = "error"
    )]
    journald_filter: String,

    /// Sets whether or not the formatter emits ANSI terminal escape codes for colors and other
    /// text formatting.
    #[arg(
        long,
        value_name = "COLOR",
        global = true,
        default_value_t = ColorMode::Always
    )]
    color: ColorMode,
}

/// Constant to convert megabytes to bytes
const MB_TO_BYTES: u64 = 1024 * 1024;

impl Logs {
    /// Builds tracing layers from the current log options.
    pub fn layers<S>(&self) -> eyre::Result<(Vec<BoxedLayer<S>>, Option<FileWorkerGuard>)>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let mut layers = Vec::new();

        if self.journald {
            layers.push(
                reth_tracing::journald(
                    EnvFilter::try_new(DEFAULT_ENV_FILTER_DIRECTIVE)?
                        .add_directive(self.journald_filter.parse()?),
                )
                .expect("Could not connect to journald"),
            );
        }

        let file_guard = if self.log_file_max_files > 0 {
            let (layer, guard) = reth_tracing::file(
                EnvFilter::try_new(DEFAULT_ENV_FILTER_DIRECTIVE)?
                    .add_directive(self.log_file_filter.parse()?),
                &self.log_file_directory,
                "reth.log",
                self.log_file_max_size * MB_TO_BYTES,
                self.log_file_max_files,
            );
            layers.push(layer);
            Some(guard)
        } else {
            None
        };

        Ok((layers, file_guard))
    }
}

/// The verbosity settings for the cli.
#[derive(Debug, Copy, Clone, Args)]
#[command(next_help_heading = "Display")]
pub struct Verbosity {
    /// Set the minimum log level.
    ///
    /// -v      Errors
    /// -vv     Warnings
    /// -vvv    Info
    /// -vvvv   Debug
    /// -vvvvv  Traces (warning: very verbose!)
    #[clap(short, long, action = ArgAction::Count, global = true, default_value_t = 3, verbatim_doc_comment, help_heading = "Display")]
    verbosity: u8,

    /// Silence all log output.
    #[clap(long, alias = "silent", short = 'q', global = true, help_heading = "Display")]
    quiet: bool,
}

impl Verbosity {
    /// Get the corresponding [Directive] for the given verbosity, or none if the verbosity
    /// corresponds to silent.
    pub fn directive(&self) -> Directive {
        if self.quiet {
            LevelFilter::OFF.into()
        } else {
            let level = match self.verbosity - 1 {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                _ => Level::TRACE,
            };

            format!("{level}").parse().unwrap()
        }
    }
}

/// The color mode for the cli.
#[derive(Debug, Copy, Clone, ValueEnum, Eq, PartialEq)]
pub enum ColorMode {
    /// Colors on
    Always,
    /// Colors on
    Auto,
    /// Colors off
    Never,
}

impl Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColorMode::Always => write!(f, "always"),
            ColorMode::Auto => write!(f, "auto"),
            ColorMode::Never => write!(f, "never"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::utils::SUPPORTED_CHAINS;
    use clap::CommandFactory;

    #[test]
    fn parse_color_mode() {
        let reth = Cli::<()>::try_parse_from(["reth", "node", "--color", "always"]).unwrap();
        assert_eq!(reth.logs.color, ColorMode::Always);
    }

    /// Tests that the help message is parsed correctly. This ensures that clap args are configured
    /// correctly and no conflicts are introduced via attributes that would result in a panic at
    /// runtime
    #[test]
    fn test_parse_help_all_subcommands() {
        let reth = Cli::<()>::command();
        for sub_command in reth.get_subcommands() {
            let err = Cli::<()>::try_parse_from(["reth", sub_command.get_name(), "--help"])
                .err()
                .unwrap_or_else(|| {
                    panic!("Failed to parse help message {}", sub_command.get_name())
                });

            // --help is treated as error, but
            // > Not a true "error" as it means --help or similar was used. The help message will be sent to stdout.
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        }
    }

    /// Tests that the log directory is parsed correctly. It's always tied to the specific chain's
    /// name
    #[test]
    fn parse_logs_path() {
        let mut reth = Cli::<()>::try_parse_from(["reth", "node"]).unwrap();
        reth.logs.log_file_directory =
            reth.logs.log_file_directory.join(reth.chain.chain.to_string());
        let log_dir = reth.logs.log_file_directory;
        let end = format!("reth/logs/{}", SUPPORTED_CHAINS[0]);
        assert!(log_dir.as_ref().ends_with(end), "{:?}", log_dir);

        let mut iter = SUPPORTED_CHAINS.iter();
        iter.next();
        for chain in iter {
            let mut reth = Cli::<()>::try_parse_from(["reth", "node", "--chain", chain]).unwrap();
            reth.logs.log_file_directory =
                reth.logs.log_file_directory.join(reth.chain.chain.to_string());
            let log_dir = reth.logs.log_file_directory;
            let end = format!("reth/logs/{}", chain);
            assert!(log_dir.as_ref().ends_with(end), "{:?}", log_dir);
        }
    }

    #[test]
    fn override_trusted_setup_file() {
        // We already have a test that asserts that this has been initialized,
        // so we cheat a little bit and check that loading a random file errors.
        let reth = Cli::<()>::try_parse_from(["reth", "node", "--trusted-setup-file", "README.md"])
            .unwrap();
        assert!(reth.run().is_err());
    }
}
