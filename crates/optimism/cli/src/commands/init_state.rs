//! Command that initializes the node from a genesis file.

use alloy_consensus::Header;
use clap::Parser;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, CliHeader, CliNodeTypes, Environment};
use reth_db_common::init::init_from_state_dump;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_primitives::{
    bedrock::{BEDROCK_HEADER, BEDROCK_HEADER_HASH},
    OpPrimitives,
};
use reth_primitives_traits::SealedHeader;
use reth_provider::{
    BlockNumReader, ChainSpecProvider, DBProvider, DatabaseProviderFactory,
    StaticFileProviderFactory, StaticFileWriter,
};
use std::{io::BufReader, sync::Arc, path::Path};
use tracing::info;
use alloy_rlp::Decodable;
use reth_node_builder::NodePrimitives;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitStateCommandOp<C: ChainSpecParser> {
    #[command(flatten)]
    init_state: reth_cli_commands::init_state::InitStateCommand<C>,

    /// Specifies whether to initialize the state without relying on OVM or EVM historical data.
    ///
    /// When enabled, and before inserting the state, it creates a dummy chain up to the last OVM
    /// block (#105235062) (14GB / 90 seconds). It then, appends the Bedrock block. This is hardcoded
    /// for OP mainnet, for other OP chains you will need to pass in a header.
    ///
    /// - **Note**: **Do not** import receipts and blocks beforehand, or this will fail or be
    ///   ignored.
    #[arg(long, default_value = "false")]
    without_ovm: bool,
}

/// Reads the header RLP from a file and returns the Header.
///
/// This supports both raw rlp bytes and rlp hex string.
pub(crate) fn read_header_from_file<H>(path: &Path) -> Result<H, eyre::Error>
where
    H: Decodable,
{
    let buf = if let Ok(content) = reth_fs_util::read_to_string(path) {
        alloy_primitives::hex::decode(content.trim())?
    } else {
        // If UTF-8 decoding fails, read as raw bytes
        reth_fs_util::read(path)?
    };

    let header = H::decode(&mut &buf[..])?;
    Ok(header)
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

        // OP-Mainnet may want to bootstrap a chain without OVM historical data
        if self.without_ovm {
            let last_block_number = provider_rw.last_block_number()?;

            if last_block_number == 0 {
                if provider_factory.chain_spec().is_optimism_mainnet() {
                    reth_cli_commands::init_state::without_evm::setup_without_evm(
                        &provider_rw,
                        SealedHeader::new(BEDROCK_HEADER, BEDROCK_HEADER_HASH),
                        |number| {
                            let mut header = Header::default();
                            header.set_number(number);
                            header
                        },
                    )?;
                } else {
                    // ensure header, total difficulty and header hash are provided
                    let header = self.init_state.header.ok_or_else(|| eyre::eyre!("Header file must be provided"))?;
                    let header: Header = read_header_from_file(&header).unwrap();

                    let header_hash = self.init_state.header_hash.unwrap_or_else(|| header.hash_slow());

                    reth_cli_commands::init_state::without_evm::setup_without_evm(
                        &provider_rw,
                        SealedHeader::new(header, header_hash),
                        |number| {
                            let mut header =
                                <<N::Primitives as NodePrimitives>::BlockHeader>::default();
                            header.set_number(number);
                            header
                        },
                    )?;
                }

                // SAFETY: it's safe to commit static files, since in the event of a crash, they
                // will be unwound according to database checkpoints.
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
