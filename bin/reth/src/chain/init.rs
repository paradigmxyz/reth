use crate::{
    dirs::{DbPath, PlatformPath},
    utils::chainspec::genesis_value_parser,
};
use clap::Parser;
use reth_primitives::ChainSpec;
use reth_staged_sync::utils::init::{init_db, init_genesis};
use std::sync::Arc;
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitCommand {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: ChainSpec,
}

impl InitCommand {
    /// Execute the `init` command
    pub async fn execute(&self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth import starting");

        info!(target: "reth::cli", path = %self.db, "Opening database");
        let db = Arc::new(init_db(&self.db)?);
        info!(target: "reth::cli", "Database opened");

        info!(target: "reth::cli", "Writing genesis block");
        let genesis_hash = init_genesis(db, self.chain.clone())?;

        if genesis_hash != self.chain.genesis_hash() {
            // TODO: better error text
            return Err(eyre::eyre!(
                "Genesis hash mismatch: expected {}, got {}",
                self.chain.genesis_hash(),
                genesis_hash
            ))
        }

        Ok(())
    }
}
