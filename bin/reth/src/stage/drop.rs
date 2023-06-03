//! Database debugging tool
use crate::{
    args::StageEnum,
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::Parser;
use reth_db::{
    database::Database,
    mdbx::{Env, WriteMap},
    tables,
    transaction::DbTxMut,
};
use reth_primitives::{stage::StageId, ChainSpec};
use reth_staged_sync::utils::{chainspec::genesis_value_parser, init::insert_genesis_state};
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

    stage: StageEnum,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        std::fs::create_dir_all(&db_path)?;

        let db = Env::<WriteMap>::open(db_path.as_ref(), reth_db::mdbx::EnvKind::RW)?;

        let tool = DbTool::new(&db)?;

        tool.db.update(|tx| {
            match &self.stage {
                StageEnum::Execution => {
                    tx.clear::<tables::PlainAccountState>()?;
                    tx.clear::<tables::PlainStorageState>()?;
                    tx.clear::<tables::AccountChangeSet>()?;
                    tx.clear::<tables::StorageChangeSet>()?;
                    tx.clear::<tables::Bytecodes>()?;
                    tx.clear::<tables::Receipts>()?;
                    tx.put::<tables::SyncStage>(
                        StageId::Execution.to_string(),
                        Default::default(),
                    )?;
                    insert_genesis_state::<Env<WriteMap>>(tx, self.chain.genesis())?;
                }
                StageEnum::Hashing => {
                    // Clear hashed accounts
                    tx.clear::<tables::HashedAccount>()?;
                    tx.put::<tables::SyncStage>(
                        StageId::AccountHashing.to_string(),
                        Default::default(),
                    )?;

                    // Clear hashed storages
                    tx.clear::<tables::HashedStorage>()?;
                    tx.put::<tables::SyncStage>(
                        StageId::StorageHashing.to_string(),
                        Default::default(),
                    )?;
                }
                StageEnum::Merkle => {
                    tx.clear::<tables::AccountsTrie>()?;
                    tx.clear::<tables::StoragesTrie>()?;
                    tx.put::<tables::SyncStage>(
                        StageId::MerkleExecute.to_string(),
                        Default::default(),
                    )?;
                    tx.put::<tables::SyncStage>(
                        StageId::MerkleUnwind.to_string(),
                        Default::default(),
                    )?;
                    tx.delete::<tables::SyncStageProgress>(
                        StageId::MerkleExecute.to_string(),
                        None,
                    )?;
                }
                StageEnum::History => {
                    tx.clear::<tables::AccountHistory>()?;
                    tx.clear::<tables::StorageHistory>()?;
                    tx.put::<tables::SyncStage>(
                        StageId::IndexAccountHistory.to_string(),
                        Default::default(),
                    )?;
                    tx.put::<tables::SyncStage>(
                        StageId::IndexStorageHistory.to_string(),
                        Default::default(),
                    )?;
                }
                _ => {
                    info!("Nothing to do for stage {:?}", self.stage);
                    return Ok(())
                }
            }

            tx.put::<tables::SyncStage>(StageId::Finish.to_string(), Default::default())?;

            Ok::<_, eyre::Error>(())
        })??;

        Ok(())
    }
}
