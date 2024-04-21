//! Command that initializes the node from a genesis file.

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_db::{
    database::Database,
    init_db, tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_core::init::{
    init_genesis, insert_genesis_hashes, insert_genesis_history, insert_genesis_state,
};
use reth_primitives::{stage::StageId, Address, ChainSpec, GenesisAccount, B256};
use reth_provider::{BlockHashReader, BlockNumReader, ChainSpecProvider, ProviderFactory};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
    sync::Arc,
};
use tracing::{debug, info};

/// Number of accounts from state dump file to insert into database at once.
///
/// Default is 10k accounts.
pub const DEFAULT_LEN_ACCOUNTS_CHUNK: usize = 10_000;

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
    /// Must contain accounts in following format, additional account fields are ignored. Can
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
    #[arg(long, value_name = "STATE_DUMP_FILE", verbatim_doc_comment, default_value = None)]
    state: Option<PathBuf>,

    #[command(flatten)]
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
        let db = Arc::new(init_db(&db_path, self.db.database_args())?);
        info!(target: "reth::cli", "Database opened");

        let provider_factory = ProviderFactory::new(db, self.chain, data_dir.static_files_path())?;

        info!(target: "reth::cli", "Writing genesis block");

        let hash = match self.state {
            Some(path) => init_at_state(path, provider_factory)?,
            None => init_genesis(provider_factory)?,
        };

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

/// Initialize chain with state at specific block, from a file with state dump.
pub fn init_at_state<DB: Database>(
    state_dump_path: PathBuf,
    factory: ProviderFactory<DB>,
) -> eyre::Result<B256> {
    let block = factory.last_block_number()?;
    let hash = factory.block_hash(block)?.unwrap();

    // Open the file in read-only mode with buffer.
    let file = File::open(state_dump_path)?;
    let mut reader = BufReader::new(file);

    debug!(target: "reth::cli",
        block,
        chain=%factory.chain_spec().chain,
        "Initializing state at block"
    );

    let mut accounts = Vec::with_capacity(10_000);
    let mut line = String::new();

    // first line can be state root, then it can be used for verifying against computed state root
    reader.read_line(&mut line)?;
    let _state_root = match serde_json::from_str::<B256>(&line) {
        Ok(root) => Some(root),
        Err(_) => {
            let GenesisAccountWithAddress { genesis_account, address } =
                serde_json::from_str(&line)?;
            accounts.push((address, genesis_account));
            line.clear();

            None
        }
    };

    // remaining lines are accounts
    while let Ok(n) = reader.read_line(&mut line) {
        if accounts.len() == DEFAULT_LEN_ACCOUNTS_CHUNK || n == 0 {
            debug!(target: "reth::cli",
                block,
                accounts=accounts.len(),
                "Writing accounts to db"
            );
            // use transaction to insert genesis header
            let provider_rw = factory.provider_rw()?;
            insert_genesis_hashes(
                &provider_rw,
                accounts.iter().map(|(address, account)| (address, account)),
            )?;
            insert_genesis_history(
                &provider_rw,
                accounts.iter().map(|(address, account)| (address, account)),
            )?;

            let tx = provider_rw.into_tx();

            insert_genesis_state::<DB>(
                &tx,
                accounts.len(),
                accounts.iter().map(|(address, account)| (address, account)),
            )?;

            // insert sync stage
            for stage in StageId::ALL.iter() {
                tx.put::<tables::StageCheckpoints>(stage.to_string(), Default::default())?;
            }

            tx.commit()?;
        }

        if n == 0 {
            break;
        }

        let GenesisAccountWithAddress { genesis_account, address } = serde_json::from_str(&line)?;
        accounts.push((address, genesis_account));

        line.clear();
    }

    Ok(hash)
}

/// An account as in the state dump file. This contains a [`GenesisAccount`] and the account's
/// address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct GenesisAccountWithAddress {
    /// The account's balance, nonce, code, and storage.
    #[serde(flatten)]
    genesis_account: GenesisAccount,
    /// The account's address.
    address: Address,
}
