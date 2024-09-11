//! Command that initializes the node from a genesis file.

use alloy_primitives::B256;
use clap::Parser;
use reth_chainspec::ChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::common::{AccessRights, Environment, EnvironmentArgs};
use reth_config::config::EtlConfig;
use reth_db_common::init::init_from_state_dump;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithEngine};
use reth_optimism_primitives::ovm::LAST_OVM_HEADER;
use reth_provider::{BlockNumReader, ChainSpecProvider, ProviderFactory};
use std::{fs::File, io::BufReader, path::PathBuf};
use tracing::info;

mod ovm;

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
    /// When enabled, and before inserting the state, it creates a dummy chain up to the last OVM block (#105235062) (14GB / 90 seconds).
    /// 
    /// - **Note**: **Do not** import receipts and blocks beforehand, or this will fail or be ignored.
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

        if provider_factory.chain_spec().is_optimism_mainnet() && self.without_ovm {
            let last_block_number = provider_factory.last_block_number()?;

            if last_block_number == 0 {
                ovm::setup_op_mainnet_without_ovm(provider_factory.clone())?;
            } else if last_block_number > 0 && last_block_number < LAST_OVM_HEADER.number {
                return Err(eyre::eyre!(
                    "Data directory should be empty when calling init-state with --without-ovm."
                ))
            }
        }

        info!(target: "reth::cli", "Initiating state dump");

        let hash = init_at_state(self.state, provider_factory, config.stages.etl)?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}

/// Initialize chain with state at specific block, from a file with state dump.
pub fn init_at_state<N: NodeTypesWithDB<ChainSpec = ChainSpec>>(
    state_dump_path: PathBuf,
    factory: ProviderFactory<N>,
    etl_config: EtlConfig,
) -> eyre::Result<B256> {
    info!(target: "reth::cli",
        path=?state_dump_path,
        "Opening state dump");

    let file = File::open(state_dump_path)?;
    let reader = BufReader::new(file);

    init_from_state_dump(reader, factory, etl_config)
}
