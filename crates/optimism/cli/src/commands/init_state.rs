//! Command that initializes the node from a genesis file.

use alloy_primitives::{B256, U256};
use clap::Parser;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, CliNodeTypes, Environment};
use reth_cli_commands::init_state::without_evm;
use reth_db_common::init::init_from_state_dump;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::OpPrimitives;
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    BlockNumReader, DatabaseProviderFactory, StaticFileProviderFactory, StaticFileWriter,
};
use std::{io::BufReader, str::FromStr, sync::Arc};
use tracing::info;

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
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec, Primitives = OpPrimitives>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } =
            self.init_state.env.init::<N>(AccessRights::RW)?;

        let static_file_provider = provider_factory.static_file_provider();
        let provider_rw = provider_factory.database_provider_rw()?;

        if self.init_state.without_evm {
            info!(target: "reth::cli", "Initializing state without EVM historical data");
            // ensure header, total difficulty and header hash are provided
            let header = self
                .init_state
                .header
                .ok_or_else(|| eyre::eyre!("Header file must be provided"))?;
            let header = without_evm::read_header_from_file(header)?;

            let header_hash = self
                .init_state
                .header_hash
                .ok_or_else(|| eyre::eyre!("Header hash must be provided"))?;
            let header_hash = B256::from_str(&header_hash)?;

            let total_difficulty = self
                .init_state
                .total_difficulty
                .ok_or_else(|| eyre::eyre!("Total difficulty must be provided"))?;
            let total_difficulty = U256::from_str(&total_difficulty)?;

            let last_block_number = provider_rw.last_block_number()?;

            if last_block_number == 0 {
                info!(target: "reth::cli", "header: {:?}, header_hash: {:?}, total_difficulty: {:?}", header, header_hash, total_difficulty);
                without_evm::setup_without_evm(
                    &provider_rw,
                    SealedHeader::new(header, header_hash),
                    total_difficulty,
                )?;

                // SAFETY: it's safe to commit static files, since in the event of a crash, they
                // will be unwound according to database checkpoints.
                //
                // Necessary to commit, so the header is accessible to provider_rw and
                // init_state_dump
                static_file_provider.commit()?;
            } else if last_block_number > 0 && last_block_number < header.number {
                return Err(eyre::eyre!(
                    "Data directory should be empty when calling init-state with --without-evm-history."
                ));
            }
        }

        info!(target: "reth::cli", "Initiating state dump");

        let reader = BufReader::new(reth_fs_util::open(self.init_state.state)?);
        let hash = init_from_state_dump(reader, &provider_rw, config.stages.etl)?;

        provider_rw.commit()?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

impl<C: ChainSpecParser> InitStateCommandOp<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        self.init_state.chain_spec()
    }
}
