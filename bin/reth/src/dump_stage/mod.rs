//! Database debugging tool

mod execution;
use execution::dump_execution_stage;

use crate::{
    db::DbTool,
    dirs::{DbPath, PlatformPath},
};
use clap::Parser;

/// `reth dump-stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

    #[clap(subcommand)]
    command: Stages,
}

/// Supported stages to be dumped
#[derive(Debug, Clone, Parser)]
pub enum Stages {
    /// Execution stage.
    Execution(StageCommand),
}

/// Stage command that takes a range
#[derive(Debug, Clone, Parser)]
pub struct StageCommand {
    /// The path to the new database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "OUTPUT_PATH", verbatim_doc_comment, default_value_t)]
    output_db: PlatformPath<DbPath>,
    /// From which block.
    #[arg(long, short)]
    from: u64,
    /// To which block.
    #[arg(long, short)]
    to: u64,
    /// If passed, it will dry-run a stage execution from the newly created database right after
    /// dumping.
    #[arg(long, short, default_value = "false")]
    dry_run: bool,
}

impl Command {
    /// Execute `dump-stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        std::fs::create_dir_all(&self.db)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            self.db.as_ref(),
            reth_db::mdbx::EnvKind::RW,
        )?;

        let mut tool = DbTool::new(&db)?;

        match &self.command {
            Stages::Execution(StageCommand { output_db, from, to, dry_run, .. }) => {
                dump_execution_stage(&mut tool, *from, *to, output_db, *dry_run).await?
            }
        }

        Ok(())
    }
}
