//! CLI definition and entrypoint to executable
use std::str::FromStr;

use crate::{
    chain, db,
    dirs::{LogsDir, PlatformPath},
    node, p2p,
    runner::CliRunner,
    stage, test_eth_chain, test_vectors,
};
use clap::{ArgAction, Args, Parser, Subcommand};
use reth_tracing::{
    tracing::{metadata::LevelFilter, Level, Subscriber},
    tracing_subscriber::{filter::Directive, registry::LookupSpan},
    BoxedLayer, FileWorkerGuard,
};

/// Parse CLI options, set up logging and run the chosen command.
pub fn run() -> eyre::Result<()> {
    let opt = Cli::parse();

    let (layer, _guard) = opt.logs.layer();
    reth_tracing::init(vec![layer, reth_tracing::stdout(opt.verbosity.directive())]);

    let runner = CliRunner::default();

    match opt.command {
        Commands::Node(command) => runner.run_command_until_exit(|ctx| command.execute(ctx)),
        Commands::Init(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Import(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Db(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::Stage(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::P2P(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::TestVectors(command) => runner.run_until_ctrl_c(command.execute()),
        Commands::TestEthChain(command) => runner.run_until_ctrl_c(command.execute()),
    }
}

/// Commands to be executed
#[derive(Subcommand)]
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
    /// P2P Debugging utilities
    #[command(name = "p2p")]
    P2P(p2p::Command),
    /// Run Ethereum blockchain tests
    #[command(name = "test-chain")]
    TestEthChain(test_eth_chain::Command),
    /// Generate Test Vectors
    #[command(name = "test-vectors")]
    TestVectors(test_vectors::Command),
}

#[derive(Parser)]
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

#[derive(Args)]
#[command(next_help_heading = "Logging")]
struct Logs {
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
    fn layer<S>(&self) -> (BoxedLayer<S>, Option<FileWorkerGuard>)
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let directive = Directive::from_str(self.filter.as_str())
            .unwrap_or_else(|_| Directive::from_str("debug").unwrap());

        if self.journald {
            (reth_tracing::journald(directive).expect("Could not connect to journald"), None)
        } else {
            let (layer, guard) = reth_tracing::file(directive, &self.log_directory, "reth.log");
            (layer, Some(guard))
        }
    }
}

#[derive(Args)]
#[command(next_help_heading = "Display")]
struct Verbosity {
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
    fn directive(&self) -> Directive {
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
