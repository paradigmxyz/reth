use clap::Args;

/// Parameters to configure Gas Price Oracle
#[derive(Debug, Args, PartialEq, Default)]
pub struct GasPriceOracleArgs {
    /// Number of recent blocks to check for gas price
    #[arg(long = "gpo.blocks")]
    pub blocks: Option<u32>,

    /// Gas Price below which gpo will ignore transactions
    #[arg(long = "gpo.ignoreprice")]
    pub ignore_price: Option<u64>,

    /// Maximum transaction priority fee(or gasprice before London Fork) to be recommended by gpo
    #[arg(long = "gpo.maxprice")]
    pub max_price: Option<u64>,

    /// The percentile of gas prices to use for the estimate
    #[arg(long = "gpo.percentile")]
    pub percentile: Option<u32>,
}
