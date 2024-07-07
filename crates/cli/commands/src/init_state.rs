//! Command that initializes the node from a genesis file.

use crate::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_config::config::EtlConfig;
use reth_db_api::database::Database;
use reth_db_common::init::init_from_state_dump;
use reth_primitives::B256;
use reth_provider::ProviderFactory;

use std::{fs::File, io::BufReader, path::PathBuf};
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommand {
    #[command(flatten)]
    env: EnvironmentArgs,

    /// JSONL file with state dump.
    ///
    /// Must contain accounts in following format, additional account fields are ignored. Must
    /// also contain { "root": \<state-root\> } as first line.
    /// {
    ///     "balance": "\<balance\>",
    ///     "nonce": \<nonce\>,
    ///     "code": "\<bytecode\>",
    ///     "storage": {
    ///         "\<key\>": "\<value\>",
    ///         ..
    ///     },
    ///     "address": "\<address\>",
    /// }
    ///
    /// Allows init at a non-genesis block. Caution! Blocks must be manually imported up until
    /// and including the non-genesis block to init chain at. See 'import' command.
    #[arg(value_name = "STATE_DUMP_FILE", verbatim_doc_comment)]
    state: PathBuf,
}

impl InitStateCommand {
    /// Execute the `init` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } = self.env.init(AccessRights::RW)?;

        info!(target: "reth::cli", "Initiating state dump");

        let hash = init_at_state(self.state, provider_factory, config.stages.etl)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

/// Initialize chain with state at specific block, from a file with state dump.
pub fn init_at_state<DB: Database>(
    state_dump_path: PathBuf,
    factory: ProviderFactory<DB>,
    etl_config: EtlConfig,
) -> eyre::Result<B256> {
    info!(target: "reth::cli",
        path=?state_dump_path,
        "Opening state dump");

    let file = File::open(state_dump_path)?;
    let reader = BufReader::new(file);

    init_from_state_dump(reader, factory, etl_config)
}
