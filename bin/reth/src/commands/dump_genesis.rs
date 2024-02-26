//! Command that dumps genesis block JSON configuration to stdout
use crate::args::utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS};
use clap::Parser;
use reth_primitives::ChainSpec;
use std::sync::Arc;

/// Dumps genesis block JSON configuration to stdout
#[derive(Debug, Parser)]
pub struct DumpGenesisCommand {
    /// Görli network: pre-configured proof-of-authority test network
    #[arg(long, verbatim_doc_comment)]
    goerli: bool,

    /// Ethereum mainnet
    #[arg(long, verbatim_doc_comment)]
    mainnet: bool,

    /// Holesky network: pre-configured proof-of-stake test network
    #[arg(long, verbatim_doc_comment)]
    holesky: bool,

    /// Sepolia network: pre-configured proof-of-work test network
    #[arg(long, verbatim_doc_comment)]
    sepolia: bool,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,
}

impl DumpGenesisCommand {
    /// Execute the `dumpgenesis` command
    pub async fn execute(self) -> eyre::Result<()> {
        let genesis = if self.mainnet {
            reth_primitives::MAINNET.genesis()
        } else if self.holesky {
            reth_primitives::HOLESKY.genesis()
        } else if self.sepolia {
            reth_primitives::SEPOLIA.genesis()
        } else if self.goerli {
            reth_primitives::GOERLI.genesis()
        } else {
            self.chain.genesis()
        };
        println!("{}", serde_json::to_string_pretty(genesis)?);
        Ok(())
    }
}
