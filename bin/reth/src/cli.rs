//! CLI definition and entrypoint to executable
use crate::{
    chain, config, db,
    dirs::{LogsDir, PlatformPath},
    drop_stage, dump_stage, merkle_debug, node, p2p,
    runner::CliRunner,
    stage, test_eth_chain, test_vectors,
};
use clap::{ArgAction, Args, Parser, Subcommand};
use reth_tracing::{
    tracing::{metadata::LevelFilter, Level, Subscriber},
    tracing_subscriber::{filter::Directive, registry::LookupSpan},
    BoxedLayer, FileWorkerGuard,
};
use std::str::FromStr;

/// Parse CLI options, set up logging and run the chosen command.
pub fn run() -> eyre::Result<()> {
    let opt = Cli::parse();

    let mut layers = vec![reth_tracing::stdout(opt.verbosity.directive())];
    if let Some((layer, _guard)) = opt.logs.layer() {
        layers.push(layer);
    }
    reth_tracing::init(layers);

    let runner = CliRunner::default();

    match opt.command {
        Commands::Node(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
        Commands::Init(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::Import(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::Db(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::Stage(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::DumpStage(command) => {
            // TODO: This should be run_blocking_until_ctrl_c as well, but fails to compile due to
            // weird compiler GAT issues.
            runner.run_until_ctrl_c(command.execute())
        }
        Commands::DropStage(command) => runner.run_blocking_until_ctrl_c(command.execute()),
        Commands::P2P(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::TestEthChain(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Config(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::MerkleDebug(command) => runner.run_until_ctrl_c(command.execute()),
    }
}

/// Commands to be executed
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Start the node
    #[command(name = "node")]
    Node(node::Command),
    /// Initialize the database from a genesis file.
    #[command(name = "init")]
    Init(chain::InitCommand),
    /// This syncs RLP encoded blocks from a file.
    #[command(name = "import")]
    Import(chain::ImportCommand),
    /// Database debugging utilities
    #[command(name = "db")]
    Db(db::Command),
    /// Run a single stage.
    ///
    /// Note that this won't use the Pipeline and as a result runs stages
    /// assuming that all the data can be held in memory. It is not recommended
    /// to run a stage for really large block ranges if your computer does not have
    /// a lot of memory to store all the data.
    #[command(name = "stage")]
    Stage(stage::Command),
    /// Dumps a stage from a range into a new database.
    #[command(name = "dump-stage")]
    DumpStage(dump_stage::Command),
    /// Drops a stage's tables from the database.
    #[command(name = "drop-stage")]
    DropStage(drop_stage::Command),
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command),
    /// Run Ethereum blockchain tests
    #[command(name = "test-chain")]
    TestEthChain(test_eth_chain::Command),
    /// Generate Test Vectors
    #[command(name = "test-vectors")]
    TestVectors(test_vectors::Command),
    /// Write config to stdout
    #[command(name = "config")]
    Config(config::Command),
    /// Debug state root calculation
    #[command(name = "merkle-debug")]
    MerkleDebug(merkle_debug::Command),
}

#[derive(Debug, Parser)]
#[command(author, version = "0.1", about = "Reth", long_about = None)]
struct Cli {
    /// The command to run
    #[clap(subcommand)]
    command: Commands,

    #[clap(flatten)]
    logs: Logs,

    #[clap(flatten)]
    verbosity: Verbosity,
}

/// The log configuration.
#[derive(Debug, Args)]
#[command(next_help_heading = "Logging")]
pub struct Logs {
    /// The flag to enable persistent logs.
    #[arg(long = "log.persistent", global = true, conflicts_with = "journald")]
    persistent: bool,

    /// The path to put log files in.
    #[arg(
        long = "log.directory",
        value_name = "PATH",
        global = true,
        default_value_t,
        conflicts_with = "journald"
    )]
    log_directory: PlatformPath<LogsDir>,

    /// Log events to journald.
    #[arg(long = "log.journald", global = true, conflicts_with = "log_directory")]
    journald: bool,

    /// The filter to use for logs written to the log file.
    #[arg(long = "log.filter", value_name = "FILTER", global = true, default_value = "debug")]
    filter: String,
}

impl Logs {
    /// Builds a tracing layer from the current log options.
    pub fn layer<S>(&self) -> Option<(BoxedLayer<S>, Option<FileWorkerGuard>)>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let directive = Directive::from_str(self.filter.as_str())
            .unwrap_or_else(|_| Directive::from_str("debug").unwrap());

        if self.journald {
            Some((reth_tracing::journald(directive).expect("Could not connect to journald"), None))
        } else if self.persistent {
            let (layer, guard) = reth_tracing::file(directive, &self.log_directory, "reth.log");
            Some((layer, Some(guard)))
        } else {
            None
        }
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

            format!("reth::cli={level}").parse().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Tests that the help message is parsed correctly. This ensures that clap args are configured
    /// correctly and no conflicts are introduced via attributes that would result in a panic at
    /// runtime
    #[test]
    fn test_parse_help_all_subcommands() {
        let reth = Cli::command();
        for sub_command in reth.get_subcommands() {
            let err = Cli::try_parse_from(["reth", sub_command.get_name(), "--help"]).unwrap_err();
            // --help is treated as error, but
            // > Not a true "error" as it means --help or similar was used. The help message will be sent to stdout.
            assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        }
    }
}
