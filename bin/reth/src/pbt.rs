//! Explicit opt-in for the reference PBT devnet backend.

use clap::Args;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_pbt::{activation_timestamp, configure_chain};
use std::sync::Arc;

#[derive(Debug, Default, Args)]
pub(crate) struct PbtArgs {
    /// Enable experimental BLAKE3 PBT state commitments using genesis config.binaryTrieTime.
    #[arg(long, conflicts_with = "snap_v2")]
    pbt: bool,
    /// Hash dense PBT state-update batches in parallel; sparse updates remain incremental.
    #[arg(long = "pbt.parallel", requires = "pbt")]
    parallel: bool,
}

impl PbtArgs {
    pub(crate) fn activation(&self, chain: &ChainSpec) -> eyre::Result<Option<u64>> {
        let activation = activation_timestamp(chain)?;
        eyre::ensure!(
            self.pbt || activation.is_none(),
            "this chain configures binaryTrieTime; enable --pbt"
        );
        if self.pbt {
            eyre::ensure!(
                activation.is_some(),
                "--pbt requires config.binaryTrieTime in the genesis file"
            );
            eyre::ensure!(
                chain.is_amsterdam_active_at_timestamp(chain.genesis_timestamp()),
                "--pbt requires Amsterdam/BAL activation at genesis for staged synchronization"
            );
        }
        Ok(activation)
    }

    pub(crate) const fn parallel(&self) -> bool {
        self.parallel
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PbtChainSpecParser;

impl ChainSpecParser for PbtChainSpecParser {
    type ChainSpec = ChainSpec;

    const SUPPORTED_CHAINS: &'static [&'static str] = EthereumChainSpecParser::SUPPORTED_CHAINS;

    fn parse(value: &str) -> eyre::Result<Arc<ChainSpec>> {
        configure_chain(EthereumChainSpecParser::parse(value)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use reth::cli::Cli;

    #[test]
    fn flag_is_opt_in_and_conflicts_with_mpt_snap_sync() {
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node"]).unwrap();
        assert!(!cli.as_node_command_mut().unwrap().ext.pbt);
        let mut cli =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt"]).unwrap();
        assert!(cli.as_node_command_mut().unwrap().ext.pbt);
        let error = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--snap.v2",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn flag_requires_a_shared_activation_and_bal_schedule() {
        let chain = EthereumChainSpecParser::parse("dev").unwrap();
        assert_eq!(PbtArgs::default().activation(&chain).unwrap(), None);
        assert_eq!(
            PbtArgs { pbt: true, ..Default::default() }.activation(&chain).unwrap_err().to_string(),
            "--pbt requires config.binaryTrieTime in the genesis file"
        );
        let mut chain = (*chain).clone();
        chain.genesis.config.extra_fields.insert("binaryTrieTime".into(), serde_json::json!(0));
        assert_eq!(
            PbtArgs::default().activation(&chain).unwrap_err().to_string(),
            "this chain configures binaryTrieTime; enable --pbt"
        );
        assert_eq!(
            PbtArgs { pbt: true, ..Default::default() }.activation(&chain).unwrap_err().to_string(),
            "--pbt requires Amsterdam/BAL activation at genesis for staged synchronization"
        );
    }

    #[test]
    fn parallel_requires_pbt_and_is_opt_in() {
        let mut cli =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt"]).unwrap();
        assert!(!cli.as_node_command_mut().unwrap().ext.parallel());
        let mut cli = Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from([
            "reth",
            "node",
            "--pbt",
            "--pbt.parallel",
        ])
        .unwrap();
        assert!(cli.as_node_command_mut().unwrap().ext.parallel());
        let error =
            Cli::<PbtChainSpecParser, PbtArgs>::try_parse_from(["reth", "node", "--pbt.parallel"])
                .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }
}
