use clap::{arg, Args};

/// Public key of the IC main net.
/// IC advices to use a hardcoded value instead of querying it to avoid main-in-the middle attacks.
const IC_MAINNET_KEY: &str = "308182301d060d2b0601040182dc7c0503010201060c2b0601040182dc7c05030201036100814c0e6ec71fab583b08bd81373c255c3c371b2e84863c98a4f1e08b74235d14fb5d9c0cd546d9685f913a0c0b2cc5341583bf4b4392e467db96d65b9bb4cb717112f8472e0d5a4d14505ffd7484b01291091c5f87b98883463f98091a0baaae";

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

    /// Canister principal
    /// Default value corresponds to testnet
    #[arg(long, value_name = "EVMC_PRINCIPAL", default_value = "4fe7g-7iaaa-aaaak-aegcq-cai")]
    pub evmc_principal: String,

    /// Root key for the IC network
    #[arg(long, value_name = "IC_ROOT_KEY", default_value = IC_MAINNET_KEY)]
    pub ic_root_key: String,
}
