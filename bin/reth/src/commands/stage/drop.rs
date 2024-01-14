//! Database debugging tool

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, StageEnum,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    init::{insert_genesis_header, insert_genesis_state},
    utils::DbTool,
};
use clap::Parser;
use reth_db::{database::Database, open_db, tables, transaction::DbTxMut, DatabaseEnv};
use reth_primitives::{fs, stage::StageId, ChainSpec};
use std::sync::Arc;
use tracing::info;

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

    #[clap(flatten)]
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

        let db = open_db(db_path.as_ref(), self.db.log_level)?;

        let tool = DbTool::new(&db, self.chain.clone())?;

        tool.db.update(|tx| {
            match &self.stage {
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
                    insert_genesis_header::<DatabaseEnv>(tx, self.chain)?;
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
                    tx.delete::<tables::SyncStageProgress>(
                        StageId::MerkleExecute.to_string(),
                        None,
                    )?;
                }
                StageEnum::AccountsHistory | StageEnum::StoragesHistory => {
                    tx.clear::<tables::AccountsHistory>()?;
                    tx.clear::<tables::StoragesHistory>()?;
                    tx.put::<tables::StageCheckpoints>(
                        StageId::IndexAccountsHistory.to_string(),
                        Default::default(),
                    )?;
                    tx.put::<tables::StageCheckpoints>(
                        StageId::IndexStoragesHistory.to_string(),
                        Default::default(),
                    )?;
                }
                StageEnum::TotalDifficulty => {
                    tx.clear::<tables::HeaderTerminalDifficulties>()?;
                    tx.put::<tables::StageCheckpoints>(
                        StageId::TotalDifficulty.to_string(),
                        Default::default(),
                    )?;
                    insert_genesis_header::<DatabaseEnv>(tx, self.chain)?;
                }
                StageEnum::TxLookup => {
                    tx.clear::<tables::TransactionHashNumbers>()?;
                    tx.put::<tables::StageCheckpoints>(
                        StageId::TransactionLookup.to_string(),
                        Default::default(),
                    )?;
                    insert_genesis_header::<DatabaseEnv>(tx, self.chain)?;
                }
                _ => {
                    info!("Nothing to do for stage {:?}", self.stage);
                    return Ok(())
                }
            }

            tx.put::<tables::StageCheckpoints>(StageId::Finish.to_string(), Default::default())?;

            Ok::<_, eyre::Error>(())
        })??;

        Ok(())
    }
}
