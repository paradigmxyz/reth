//! Database debugging tool
use crate::{
    dirs::{DbPath, PlatformPath},
    utils::DbTool,
    StageEnum,
};
use clap::Parser;
use reth_db::{database::Database, tables, transaction::DbTxMut};
use reth_stages::stages::EXECUTION;
use tracing::info;

/// `reth db` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(global = true, long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

    stage: StageEnum,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(&self) -> eyre::Result<()> {
        std::fs::create_dir_all(&self.db)?;

        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            self.db.as_ref(),
            reth_db::mdbx::EnvKind::RW,
        )?;

        let tool = DbTool::new(&db)?;

        match &self.stage {
            StageEnum::Execution => {
                tool.db.update(|tx| {
                    tx.clear::<tables::PlainAccountState>()?;
                    tx.clear::<tables::PlainStorageState>()?;
                    tx.clear::<tables::AccountChangeSet>()?;
                    tx.clear::<tables::StorageChangeSet>()?;
                    tx.clear::<tables::Bytecodes>()?;
                    tx.put::<tables::SyncStage>(EXECUTION.0.to_string(), 0)?;
                    Ok::<_, eyre::Error>(())
                })??;
            }
            _ => {
                info!("Nothing to do for stage {:?}", self.stage);
            }
        }

        Ok(())
    }
}
