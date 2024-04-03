//! Database debugging tool

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, StageEnum,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::Parser;
use itertools::Itertools;
use reth_db::{open_db, static_file::iter_static_files, tables, transaction::DbTxMut, DatabaseEnv};
use reth_node_core::init::{insert_genesis_header, insert_genesis_history, insert_genesis_state};
use reth_primitives::{
    fs, stage::StageId, static_file::find_fixed_range, ChainSpec, StaticFileSegment,
};
use reth_provider::{providers::StaticFileWriter, ProviderFactory};
use std::sync::Arc;

/// `reth drop-stage` command
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

    stage: StageEnum,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;

        let db = open_db(db_path.as_ref(), self.db.database_args())?;
        let provider_factory =
            ProviderFactory::new(db, self.chain.clone(), data_dir.static_files_path())?;
        let static_file_provider = provider_factory.static_file_provider();

        let tool = DbTool::new(provider_factory, self.chain.clone())?;

        let static_file_segment = match self.stage {
            StageEnum::Headers => Some(StaticFileSegment::Headers),
            StageEnum::Bodies => Some(StaticFileSegment::Transactions),
            StageEnum::Execution => Some(StaticFileSegment::Receipts),
            _ => None,
        };

        // Delete static file segment data before inserting the genesis header below
        if let Some(static_file_segment) = static_file_segment {
            let static_file_provider = tool.provider_factory.static_file_provider();
            let static_files = iter_static_files(static_file_provider.directory())?;
            if let Some(segment_static_files) = static_files.get(&static_file_segment) {
                // Delete static files from the highest to the lowest block range
                for (block_range, _) in segment_static_files
                    .iter()
                    .sorted_by_key(|(block_range, _)| block_range.start())
                    .rev()
                {
                    static_file_provider
                        .delete_jar(static_file_segment, find_fixed_range(block_range.start()))?;
                }
            }
        }

        let provider_rw = tool.provider_factory.provider_rw()?;
        let tx = provider_rw.tx_ref();

        match self.stage {
            StageEnum::Headers => {
                tx.clear::<tables::CanonicalHeaders>()?;
                tx.clear::<tables::Headers>()?;
                tx.clear::<tables::HeaderTerminalDifficulties>()?;
                tx.clear::<tables::HeaderNumbers>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::Headers.to_string(),
                    Default::default(),
                )?;
                insert_genesis_header::<DatabaseEnv>(tx, &static_file_provider, self.chain)?;
            }
            StageEnum::Bodies => {
                tx.clear::<tables::BlockBodyIndices>()?;
                tx.clear::<tables::Transactions>()?;
                tx.clear::<tables::TransactionBlocks>()?;
                tx.clear::<tables::BlockOmmers>()?;
                tx.clear::<tables::BlockWithdrawals>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::Bodies.to_string(),
                    Default::default(),
                )?;
                insert_genesis_header::<DatabaseEnv>(tx, &static_file_provider, self.chain)?;
            }
            StageEnum::Senders => {
                tx.clear::<tables::TransactionSenders>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::SenderRecovery.to_string(),
                    Default::default(),
                )?;
            }
            StageEnum::Execution => {
                tx.clear::<tables::PlainAccountState>()?;
                tx.clear::<tables::PlainStorageState>()?;
                tx.clear::<tables::AccountChangeSets>()?;
                tx.clear::<tables::StorageChangeSets>()?;
                tx.clear::<tables::Bytecodes>()?;
                tx.clear::<tables::Receipts>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::Execution.to_string(),
                    Default::default(),
                )?;
                insert_genesis_state::<DatabaseEnv>(tx, self.chain.genesis())?;
            }
            StageEnum::AccountHashing => {
                tx.clear::<tables::HashedAccounts>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::AccountHashing.to_string(),
                    Default::default(),
                )?;
            }
            StageEnum::StorageHashing => {
                tx.clear::<tables::HashedStorages>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::StorageHashing.to_string(),
                    Default::default(),
                )?;
            }
            StageEnum::Hashing => {
                // Clear hashed accounts
                tx.clear::<tables::HashedAccounts>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::AccountHashing.to_string(),
                    Default::default(),
                )?;

                // Clear hashed storages
                tx.clear::<tables::HashedStorages>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::StorageHashing.to_string(),
                    Default::default(),
                )?;
            }
            StageEnum::Merkle => {
                tx.clear::<tables::AccountsTrie>()?;
                tx.clear::<tables::StoragesTrie>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::MerkleExecute.to_string(),
                    Default::default(),
                )?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::MerkleUnwind.to_string(),
                    Default::default(),
                )?;
                tx.delete::<tables::StageCheckpointProgresses>(
                    StageId::MerkleExecute.to_string(),
                    None,
                )?;
            }
            StageEnum::AccountHistory | StageEnum::StorageHistory => {
                tx.clear::<tables::AccountsHistory>()?;
                tx.clear::<tables::StoragesHistory>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::IndexAccountHistory.to_string(),
                    Default::default(),
                )?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::IndexStorageHistory.to_string(),
                    Default::default(),
                )?;
                insert_genesis_history(&provider_rw, &self.chain.genesis)?;
            }
            StageEnum::TxLookup => {
                tx.clear::<tables::TransactionHashNumbers>()?;
                tx.put::<tables::StageCheckpoints>(
                    StageId::TransactionLookup.to_string(),
                    Default::default(),
                )?;
                insert_genesis_header::<DatabaseEnv>(tx, &static_file_provider, self.chain)?;
            }
        }

        tx.put::<tables::StageCheckpoints>(StageId::Finish.to_string(), Default::default())?;

        static_file_provider.commit()?;
        provider_rw.commit()?;

        Ok(())
    }
}
