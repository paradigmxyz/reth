//! Main db command
//!
//! Database debugging tool

use clap::Parser;
use std::path::Path;

/// `reth stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to database folder
    #[arg(long, default_value = "~/.reth/db")]
    db: String,

    /// The name of the stage to run
    stage: String,
}

impl Command {
    /// Execute `stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let path = shellexpand::full(&self.db)?.into_owned();
        let expanded_db_path = Path::new(&path);
        std::fs::create_dir_all(expanded_db_path)?;

        let _db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            expanded_db_path,
            reth_db::mdbx::EnvKind::RW,
        )?;

        Ok(())
    }
}
