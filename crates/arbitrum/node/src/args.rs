use clap::Args;

#[derive(Debug, Clone, PartialEq, Eq, Default, Args)]
#[command(next_help_heading = "Rollup")]
pub struct RollupArgs {
    #[arg(long = "rollup.compute-pending-block")]
    pub compute_pending_block: bool,
}
