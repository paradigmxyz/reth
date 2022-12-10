//! Main node command
//!
//! Starts the client

use clap::Parser;
use std::{path::Path, sync::Arc};
use tracing::info;

/// Execute Ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to database folder
    #[arg(long, default_value = "~/.reth/db")]
    db: String,
}

impl Command {
    /// Execute `node` command
    pub async fn execute(&self) -> eyre::Result<()> {
        info!("Rust Ethereum client");

        info!("Initialize components:");

        let path = shellexpand::full(&self.db)?.into_owned();
        let expanded_db_path = Path::new(&path);
        std::fs::create_dir_all(expanded_db_path)?;
        let db = Arc::new(reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            expanded_db_path,
            reth_db::mdbx::EnvKind::RW,
        )?);
        info!("DB opened");

        // let _p2p = ();
        // let _consensus = ();
        // let _rpc = ();

        let mut pipeline = reth_stages::Pipeline::new();

        // define all stages here

        // run pipeline
        info!("Pipeline started:");
        pipeline.run(db.clone()).await?;

        info!("Finishing");
        Ok(())
    }
}
