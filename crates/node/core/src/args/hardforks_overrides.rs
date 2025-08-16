use clap::Args;
use reth_ethereum_forks::EthereumHardfork;

/// CLI args to override Ethereum hardfork activations.
#[derive(Debug, Clone, Default, Args, PartialEq, Eq)]
#[command(next_help_heading = "Hardfork Overrides")]
pub struct EthereumHardforkOverrideArgs {
    /// Override Prague hardfork activation timestamp
    #[arg(long = "override.prague", value_name = "TIMESTAMP")]
    pub prague: Option<u64>,
}

impl EthereumHardforkOverrideArgs {
    /// Get the overrides as a vector of (hardfork, timestamp) pairs.
    pub fn as_vec(&self) -> Vec<(EthereumHardfork, u64)> {
        let mut overrides = Vec::new();
        if let Some(ts) = self.prague {
            overrides.push((EthereumHardfork::Prague, ts));
        }
        overrides
    }
}
