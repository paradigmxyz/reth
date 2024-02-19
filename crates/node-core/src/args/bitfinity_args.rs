use clap::{arg, Args};

/// Bitfinity Related Args
#[derive(Debug, Args, PartialEq, Default, Clone)]
#[clap(next_help_heading = "Bitfinity Args")]
pub struct BitfinityArgs {
    /// Remote node to connect to
    #[arg(long, short = 'r', value_name = "BITFINITY_RPC_URL")]
    pub rpc_url: String,

    /// End Block
    #[arg(long, short = 'e', value_name = "END_BLOCK")]
    pub end_block: Option<u64>,

    /// Interval for importing blocks
    /// Default: 30s
    #[arg(long, short = 'i', value_name = "IMPORT_INTERVAL", default_value = "30")]
    pub import_interval: u64,

    /// Batch size for importing blocks
    /// Default: 500
    #[arg(long, short = 'b', value_name = "BATCH_SIZE", default_value = "500")]
    pub batch_size: usize,
}
