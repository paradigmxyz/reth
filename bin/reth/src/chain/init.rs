use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    init::init_genesis,
};
use clap::Parser;
use reth_db::init_db;
use reth_primitives::{
    ChainSpec,
    ephemery::{get_current_id, get_iteration, is_ephemery},
};
use std::sync::Arc;
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitCommand {
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
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    /// - holesky
    /// - ephemery
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,
}

impl InitCommand {
    /// Execute the `init` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth init starting");

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path, self.db.log_level)?);
        info!(target: "reth::cli", "Database opened");
        if is_ephemery(self.chain.chain) {
            info!(
                target: "reth::cli", 
                "The Ephemery testnet has been loaded. Current iteration: {} Current chain ID: {}", 
                get_iteration()?,
                get_current_id()
            );
        }

        info!(target: "reth::cli", "Writing genesis block");
        let hash = init_genesis(db, self.chain)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
