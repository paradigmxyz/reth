//! Command that initializes the node from a genesis file.

use crate::common::{AccessRights, Environment, EnvironmentArgs};
use alloy_primitives::B256;
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_config::config::EtlConfig;
use reth_db_common::init::init_from_state_dump;
use reth_node_builder::NodeTypesWithEngine;
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};

use std::{fs::File, io::BufReader, path::PathBuf};
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommand<C: ChainSpecParser> {
    #[command(flatten)]
    pub env: EnvironmentArgs<C>,

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
    pub state: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> InitStateCommand<C> {
    /// Execute the `init` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        info!(target: "reth::cli", "Initiating state dump");

        let hash = init_at_state(self.state, provider_factory, config.stages.etl)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

/// Initialize chain with state at specific block, from a file with state dump.
pub fn init_at_state<N: ProviderNodeTypes>(
    state_dump_path: PathBuf,
    factory: ProviderFactory<N>,
    etl_config: EtlConfig,
) -> eyre::Result<B256> {
    info!(target: "reth::cli",
        path=?state_dump_path,
        "Opening state dump");

    let file = File::open(state_dump_path)?;
    let reader = BufReader::new(file);

    let provider_rw = factory.provider_rw()?;
    let hash = init_from_state_dump(reader, &provider_rw.0, etl_config)?;
    provider_rw.commit()?;

    Ok(hash)
}
