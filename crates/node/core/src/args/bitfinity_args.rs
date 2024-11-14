use clap::{arg, Args};

/// Public key of the IC main net.
/// IC advices to use a hardcoded value instead of querying it to avoid main-in-the middle attacks.
pub const IC_MAINNET_KEY: &str = "308182301d060d2b0601040182dc7c0503010201060c2b0601040182dc7c05030201036100814c0e6ec71fab583b08bd81373c255c3c371b2e84863c98a4f1e08b74235d14fb5d9c0cd546d9685f913a0c0b2cc5341583bf4b4392e467db96d65b9bb4cb717112f8472e0d5a4d14505ffd7484b01291091c5f87b98883463f98091a0baaae";

/// Bitfinity Related Args
#[derive(Debug, Args, PartialEq, Default, Clone)]
#[clap(next_help_heading = "Bitfinity Args")]
pub struct BitfinityImportArgs {
    /// Remote node to connect to
    #[arg(long, short = 'r', value_name = "BITFINITY_RPC_URL")]
    pub rpc_url: String,

    /// Optional RPC URL where the send_raw_transaction requests are forwarded.
    /// If not provided, the RPC URL will be used.
    #[arg(long)]
    pub send_raw_transaction_rpc_url: Option<String>,

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

    /// Sets the number of block to fetch on each block importer run
    /// Default: 10_000
    #[arg(long, value_name = "MAX_FETCH_BLOCKS", default_value = "10000")]
    pub max_fetch_blocks: u64,

    /// Canister principal
    /// Default value corresponds to testnet
    #[arg(long, value_name = "EVMC_PRINCIPAL", default_value = "4fe7g-7iaaa-aaaak-aegcq-cai")]
    pub evmc_principal: String,

    /// Root key for the IC network
    #[arg(long, value_name = "IC_ROOT_KEY", default_value = IC_MAINNET_KEY)]
    pub ic_root_key: String,

}

/// Bitfinity Related Args
#[derive(Debug, Args, PartialEq, Default, Clone)]
#[clap(next_help_heading = "Bitfinity Args")]
pub struct BitfinityResetEvmStateArgs {

    /// Canister principal
    /// Default value corresponds to testnet
    #[arg(long, default_value = "4fe7g-7iaaa-aaaak-aegcq-cai")]
    pub evmc_principal: String,

    /// Path to an identity PEM file to perform state recovery IC calls.
    /// The identity must have permissions to stop the EVM canister and to
    /// update the blockchain.
    #[arg(long)]
    pub ic_identity_file_path: std::path::PathBuf,

    /// Network url
    /// This is the URL of the IC network.
    /// E.g. 
    /// - https://ic0.app
    /// - http://127.0.0.1:3333
    #[arg(long)]
    pub evm_network: String,

    /// URL used to fetch the ChainSpec information.
    /// This is usually the URL of the Bitfinity EVM block extractor.
    #[arg(long)]
    pub evm_datasource_url: String,

    /// Number of parallel requests to send data to the IC.
    #[arg(long, default_value = "4")]
    pub parallel_requests: usize,
}