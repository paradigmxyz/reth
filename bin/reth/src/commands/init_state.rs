//! Command that initializes the node from a genesis file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_config::config::EtlConfig;
use reth_db::{database::Database, init_db};
use reth_node_core::init::init_from_state_dump;
use reth_primitives::{ChainSpec, B256};
use reth_provider::ProviderFactory;

use std::{fs::File, io::BufReader, path::PathBuf, sync::Arc};
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommand {
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

    /// JSONL file with state dump.
    ///
    /// Must contain accounts in following format, additional account fields are ignored. Must
    /// also contain { "root": \<state-root\> } as first line.
    /// {
    ///     "balance": "\<balance\>",
    ///     "nonce": \<nonce\>,
    ///     "code": "\<bytecode\>",
    ///     "storage": {
    ///         "\<key\>": "\<value\>",
    ///         ..
    ///     },
    ///     "address": "\<address\>",
    /// }
    ///
    /// Allows init at a non-genesis block. Caution! Blocks must be manually imported up until
    /// and including the non-genesis block to init chain at. See 'import' command.
    #[arg(value_name = "STATE_DUMP_FILE", verbatim_doc_comment)]
    state: PathBuf,

    #[command(flatten)]
    db: DatabaseArgs,
}

impl InitStateCommand {
    /// Execute the `init` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        let db = Arc::new(init_db(&db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");

        let provider_factory = ProviderFactory::new(db, self.chain, data_dir.static_files())?;
        let etl_config = EtlConfig::new(
            Some(EtlConfig::from_datadir(data_dir.data_dir())),
            EtlConfig::default_file_size(),
        );

        info!(target: "reth::cli", "Initiating state dump");

        let hash = init_at_state(self.state, provider_factory, etl_config)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

/// Initialize chain with state at specific block, from a file with state dump.
pub fn init_at_state<DB: Database>(
    state_dump_path: PathBuf,
    factory: ProviderFactory<DB>,
    etl_config: EtlConfig,
) -> eyre::Result<B256> {
    info!(target: "reth::cli",
        path=?state_dump_path,
        "Opening state dump");

    let file = File::open(state_dump_path)?;
    let reader = BufReader::new(file);

    init_from_state_dump(reader, factory, etl_config)
}
