//! clap [Args](clap::Args) for op-reth rollup configuration

/// Parameters for rollup configuration
#[derive(Debug, clap::Args)]
#[command(next_help_heading = "Rollup")]
pub struct RollupArgs {
    /// HTTP endpoint for the sequencer mempool
    #[arg(long = "rollup.sequencerhttp", value_name = "HTTP_URL")]
    pub sequencer_http: Option<String>,

    /// Disable transaction pool gossip
    #[arg(long = "rollup.disabletxpoolgossip")]
    pub disable_txpool_gossip: bool,
}
