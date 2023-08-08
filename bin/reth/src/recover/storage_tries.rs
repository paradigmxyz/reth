use crate::{
    args::utils::genesis_value_parser,
    dirs::{DataDirPath, MaybePlatformPath},
    init::init_genesis,
    runner::CliContext,
};
use clap::Parser;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRW},
    init_db, tables,
    transaction::DbTx,
};
use reth_primitives::{keccak256, ChainSpec};
use reth_provider::{AccountExtReader, BlockNumReader, ProviderFactory};
use std::{fs, sync::Arc};
use tracing::*;

/// `reth recover storage-tries` command
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
    chain: Arc<ChainSpec>,

    /// The number of blocks in the past to look through.
    #[arg(long, default_value_t = 100)]
    lookback: u64,
}

impl Command {
    /// Execute `storage-tries` recovery command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path, None)?);

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");
        init_genesis(db.clone(), self.chain.clone())?;

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let mut provider = factory.provider_rw()?;

        let best_block = provider.best_block_number()?;

        let block_range = best_block.saturating_sub(self.lookback)..=best_block;
        let changed_accounts = provider.changed_accounts_with_range(block_range)?;
        let destroyed_accounts = provider
            .basic_accounts(changed_accounts)?
            .into_iter()
            .filter_map(|(address, acc)| acc.is_none().then_some(address))
            .collect::<Vec<_>>();

        info!(target: "reth::cli", destroyed = destroyed_accounts.len(), "Starting recovery of storage tries");

        let mut deleted_tries = 0;
        let tx_mut = provider.tx_mut();
        let mut storage_trie_cursor = tx_mut.cursor_dup_read::<tables::StoragesTrie>()?;
        for address in destroyed_accounts {
            let hashed_address = keccak256(address);
            if storage_trie_cursor.seek_exact(hashed_address)?.is_some() {
                deleted_tries += 1;
                trace!(target: "reth::cli", ?address, ?hashed_address, "Deleting storage trie");
                storage_trie_cursor.delete_current_duplicates()?;
            }
        }

        provider.commit()?;
        info!(target: "reth::cli", deleted = deleted_tries, "Finished recovery");

        Ok(())
    }
}
