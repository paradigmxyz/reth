//! Command that initializes the node from a genesis file.

use crate::common::{AccessRights, Environment, EnvironmentArgs};
use alloy_primitives::{B256, U256};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db_common::init::init_from_state_dump;
use reth_node_builder::NodeTypesWithEngine;
use reth_primitives::SealedHeader;
use reth_provider::{
    BlockNumReader, DatabaseProviderFactory, StaticFileProviderFactory, StaticFileWriter,
};

use std::{fs::File, io::BufReader, path::PathBuf, str::FromStr};
use tracing::info;

pub mod without_evm;

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

    /// Specifies whether to initialize the state without relying on EVM historical data.
    ///
    /// When enabled, and before inserting the state, it creates a dummy chain up to the last EVM
    /// block specified. It then, appends the first block provided block.
    ///
    /// - **Note**: **Do not** import receipts and blocks beforehand, or this will fail or be
    ///   ignored.
    #[arg(long, default_value = "false")]
    pub without_evm: bool,

    /// Header file containing the header in an RLP encoded format.
    #[arg(long, value_name = "HEADER_FILE", verbatim_doc_comment)]
    pub header: Option<PathBuf>,

    /// Total difficulty of the header.
    #[arg(long, value_name = "TOTAL_DIFFICULTY", verbatim_doc_comment)]
    pub total_difficulty: Option<String>,

    /// Hash of the header.
    #[arg(long, value_name = "HEADER_HASH", verbatim_doc_comment)]
    pub header_hash: Option<String>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> InitStateCommand<C> {
    /// Execute the `init` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Reth init-state starting");

        let Environment { config, provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

        let static_file_provider = provider_factory.static_file_provider();
        let provider_rw = provider_factory.database_provider_rw()?;

        if self.without_evm {
            // ensure header, total difficulty and header hash are provided
            let header = self.header.ok_or_else(|| eyre::eyre!("Header file must be provided"))?;
            let header = without_evm::read_header_from_file(header)?;

            let header_hash =
                self.header_hash.ok_or_else(|| eyre::eyre!("Header hash must be provided"))?;
            let header_hash = B256::from_str(&header_hash)?;

            let total_difficulty = self
                .total_difficulty
                .ok_or_else(|| eyre::eyre!("Total difficulty must be provided"))?;
            let total_difficulty = U256::from_str(&total_difficulty)?;

            let last_block_number = provider_rw.last_block_number()?;

            if last_block_number == 0 {
                without_evm::setup_without_evm(
                    &provider_rw,
                    &static_file_provider,
                    // &header,
                    // header_hash,
                    SealedHeader::new(header, header_hash),
                    total_difficulty,
                )?;

                // SAFETY: it's safe to commit static files, since in the event of a crash, they
                // will be unwinded according to database checkpoints.
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

        let file = File::open(self.state)?;
        let reader = BufReader::new(file);

        let hash = init_from_state_dump(reader, &provider_rw, config.stages.etl)?;

        provider_rw.commit()?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
