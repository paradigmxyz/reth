//! Command that initializes the node from a genesis file.

use clap::Parser;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment};
use reth_db_common::init::init_from_state_dump;
use reth_node_builder::NodeTypesWithEngine;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::bedrock::BEDROCK_HEADER;
use reth_provider::{
    BlockNumReader, ChainSpecProvider, DatabaseProviderFactory, StaticFileProviderFactory,
    StaticFileWriter,
};
use std::{fs::File, io::BufReader};
use tracing::info;

mod bedrock;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommandOp<C: ChainSpecParser> {
    #[command(flatten)]
    init_state: reth_cli_commands::init_state::InitStateCommand<C>,

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

impl<C: ChainSpecParser<ChainSpec = OpChainSpec>> InitStateCommandOp<C> {
    /// Execute the `init` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } =
            self.init_state.env.init::<N>(AccessRights::RW)?;

        let static_file_provider = provider_factory.static_file_provider();
        let provider_rw = provider_factory.database_provider_rw()?;

        // OP-Mainnet may want to bootstrap a chain without OVM historical data
        if provider_factory.chain_spec().is_optimism_mainnet() && self.without_ovm {
            let last_block_number = provider_rw.last_block_number()?;

            if last_block_number == 0 {
                bedrock::setup_op_mainnet_without_ovm(&provider_rw, &static_file_provider)?;

                // SAFETY: it's safe to commit static files, since in the event of a crash, they
                // will be unwinded according to database checkpoints.
                //
                // Necessary to commit, so the BEDROCK_HEADER is accessible to provider_rw and
                // init_state_dump
                static_file_provider.commit()?;
            } else if last_block_number > 0 && last_block_number < BEDROCK_HEADER.number {
                return Err(eyre::eyre!(
                    "Data directory should be empty when calling init-state with --without-ovm."
                ))
            }
        }

        info!(target: "reth::cli", "Initiating state dump");

        let reader = BufReader::new(File::open(self.init_state.state)?);
        let hash = init_from_state_dump(reader, &provider_rw, config.stages.etl)?;

        provider_rw.commit()?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
