// TODO: Remove
#![allow(missing_docs)]

//! Main db command
//!
//! Database debugging tool

use clap::{Parser, Subcommand};
use std::{path::Path, sync::Arc};
use tracing::info;

/// Execute Ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to database folder
    #[arg(long, default_value = "~/.reth/db")]
    db: String,

    #[clap(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug)]
pub enum Subcommands {
    Get(GetArgs),
}

#[derive(Parser, Debug)]
pub struct GetArgs {}

impl Command {
    /// Execute `node` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let path = shellexpand::full(&self.db)?.into_owned();
        let expanded_db_path = Path::new(&path);
        std::fs::create_dir_all(expanded_db_path)?;
        let db = Arc::new(reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            expanded_db_path,
            reth_db::mdbx::EnvKind::RW,
        )?);

        dbg!(&self);

        Ok(())
    }
}
