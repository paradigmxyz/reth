//! clap [Args](clap::Args) for op-reth rollup configuration

/// Parameters for rollup configuration
#[derive(Debug, Default, PartialEq, Eq, clap::Args)]
#[clap(next_help_heading = "Rollup")]
pub struct RollupArgs {
    /// HTTP endpoint for the sequencer mempool
    #[arg(long = "rollup.sequencer-http", value_name = "HTTP_URL")]
    pub sequencer_http: Option<String>,

    /// Disable transaction pool gossip
    #[arg(long = "rollup.disable-tx-pool-gossip")]
    pub disable_txpool_gossip: bool,

    /// Enable walkback to genesis on startup. This is useful for re-validating the existing DB
    /// prior to beginning normal syncing.
    #[arg(long = "rollup.enable-genesis-walkback")]
    pub enable_genesis_walkback: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Args, Parser};

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_database_args() {
        let default_args = RollupArgs::default();
        let args = CommandParser::<RollupArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
