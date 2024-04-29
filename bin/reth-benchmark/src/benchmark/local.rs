//! Runs the `reth benchmark` using a local database and static file folder.

use clap::Parser;
use reth_cli_runner::CliContext;
use reth_db::init_db;
use reth_node_core::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        BenchmarkArgs, DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use reth_primitives::ChainSpec;
use reth_provider::ProviderFactory;
use std::{fs, sync::Arc};

/// `reth benchmark from-local` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[command(flatten)]
    db: DatabaseArgs,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark from-local` command
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db();
        fs::create_dir_all(&db_path)?;

        // initialize the database
        let db = Arc::new(init_db(db_path, self.db.database_args())?);
        let factory = ProviderFactory::new(&db, self.chain, data_dir.static_files());
        let provider_rw = factory?.provider_rw()?;

        // TODO: Implement payloadstream from local db, based on range config

        // TODO: reusable method for payload stream + rpc url + benchmark config -> run the
        // benchmark
        Ok(())
    }
}
