//! Command that initializes the node from a genesis file.

use clap::Parser;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_db_common::init::init_from_state_dump;
use reth_node_builder::NodeTypesWithEngine;
use reth_optimism_primitives::bedrock::BEDROCK_HEADER;
use reth_provider::{
    writer::UnifiedStorageWriter, BlockNumReader, ChainSpecProvider, StaticFileProviderFactory,
};
use std::{fs::File, io::BufReader, path::PathBuf};
use tracing::info;

mod bedrock;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

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

    /// **Optimism Mainnet Only**
    ///
    /// Specifies whether to initialize the state without relying on OVM historical data.
    ///
    /// When enabled, and before inserting the state, it creates a dummy chain up to the last OVM
    /// block (#105235062) (14GB / 90 seconds). It then, appends the Bedrock block.
    ///
    /// - **Note**: **Do not** import receipts and blocks beforehand, or this will fail or be
    ///   ignored.
    #[arg(long, default_value = "false")]
    without_ovm: bool,
}

impl<C: ChainSpecParser<ChainSpec = ChainSpec>> InitStateCommand<C> {
    /// Execute the `init` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        let provider_rw = provider_factory.provider_rw()?;

        // OP-Mainnet may want to bootstrap a chain without OVM historical data
        if provider_factory.chain_spec().is_optimism_mainnet() && self.without_ovm {
            let last_block_number = provider_rw.last_block_number()?;

            if last_block_number == 0 {
                bedrock::setup_op_mainnet_without_ovm(&provider_rw)?;
            } else if last_block_number > 0 && last_block_number < BEDROCK_HEADER.number {
                return Err(eyre::eyre!(
                    "Data directory should be empty when calling init-state with --without-ovm."
                ))
            }
        }

        info!(target: "reth::cli", "Initiating state dump");

        let reader = BufReader::new(File::open(self.state)?);
        let hash = init_from_state_dump(reader, &provider_rw, config.stages.etl)?;

        UnifiedStorageWriter::commit(provider_rw, provider_factory.static_file_provider())?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
