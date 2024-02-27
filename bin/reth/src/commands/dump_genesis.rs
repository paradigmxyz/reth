//! Command that dumps genesis block JSON configuration to stdout
use crate::args::utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS};
use clap::Parser;
use reth_primitives::ChainSpec;
use std::sync::Arc;

/// Dumps genesis block JSON configuration to stdout
#[derive(Debug, Parser)]
pub struct DumpGenesisCommand {
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
    /// Execute the `dump-genesis` command
    pub async fn execute(self) -> eyre::Result<()> {
        println!("{}", serde_json::to_string_pretty(self.chain.genesis())?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dump_genesis_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: DumpGenesisCommand =
                DumpGenesisCommand::parse_from(["reth", "--chain", chain]);
            assert_eq!(
                Ok(args.chain.chain),
                chain.parse::<reth_primitives::Chain>(),
                "failed to parse chain {chain}"
            );
        }
    }
}
