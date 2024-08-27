//! Command that dumps genesis block JSON configuration to stdout
use clap::Parser;
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::args::utils::chain_help;
use std::sync::Arc;

/// Dumps genesis block JSON configuration to stdout
#[derive(Debug, Parser)]
pub struct DumpGenesisCommand<C: ChainSpecParser> {
    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = C::SUPPORTED_CHAINS[0],
        value_parser = C::default()
    )]
    chain: C::Value,
}

impl<C: ChainSpecParser> DumpGenesisCommand<C> {
    /// Execute the `dump-genesis` command
    pub async fn execute(self) -> eyre::Result<()> {
        println!("{}", serde_json::to_string_pretty(self.chain.genesis())?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_node_core::args::utils::{DefaultChainSpecParser, SUPPORTED_CHAINS};

    #[test]
    fn parse_dump_genesis_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: DumpGenesisCommand<DefaultChainSpecParser> =
                DumpGenesisCommand::parse_from(["reth", "--chain", chain]);
            assert_eq!(
                Ok(args.chain.chain),
                chain.parse::<reth_chainspec::Chain>(),
                "failed to parse chain {chain}"
            );
        }
    }
}
