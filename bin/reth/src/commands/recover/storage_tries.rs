use crate::{
    args::utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
    core::cli::runner::CliContext,
    dirs::{DataDirPath, MaybePlatformPath},
};
use clap::Parser;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRW},
    init_db, tables,
    transaction::DbTx,
};
use reth_node_core::init::init_genesis;
use reth_primitives::ChainSpec;
use reth_provider::{BlockNumReader, HeaderProvider, ProviderError, ProviderFactory};
use reth_trie::StateRoot;
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
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,
}

impl Command {
    /// Execute `storage-tries` recovery command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path, Default::default())?);

        debug!(target: "reth::cli", chain=%self.chain.chain, genesis=?self.chain.genesis_hash(), "Initializing genesis");
        init_genesis(db.clone(), self.chain.clone())?;

        let factory = ProviderFactory::new(&db, self.chain);
        let mut provider = factory.provider_rw()?;
        let best_block = provider.best_block_number()?;
        let best_header = provider
            .sealed_header(best_block)?
            .ok_or(ProviderError::HeaderNotFound(best_block.into()))?;

        let mut deleted_tries = 0;
        let tx_mut = provider.tx_mut();
        let mut hashed_account_cursor = tx_mut.cursor_read::<tables::HashedAccount>()?;
        let mut storage_trie_cursor = tx_mut.cursor_dup_read::<tables::StoragesTrie>()?;
        let mut entry = storage_trie_cursor.first()?;

        info!(target: "reth::cli", "Starting pruning of storage tries");
        while let Some((hashed_address, _)) = entry {
            if hashed_account_cursor.seek_exact(hashed_address)?.is_none() {
                deleted_tries += 1;
                storage_trie_cursor.delete_current_duplicates()?;
            }

            entry = storage_trie_cursor.next()?;
        }

        let state_root = StateRoot::from_tx(tx_mut).root()?;
        if state_root != best_header.state_root {
            eyre::bail!(
                "Recovery failed. Incorrect state root. Expected: {:?}. Received: {:?}",
                best_header.state_root,
                state_root
            );
        }

        provider.commit()?;
        info!(target: "reth::cli", deleted = deleted_tries, "Finished recovery");

        Ok(())
    }
}
