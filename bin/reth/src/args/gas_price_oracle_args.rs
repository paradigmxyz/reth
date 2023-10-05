use clap::Args;

/// Parameters to configure Gas Price Oracle
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Gas Price Oracle")]
pub struct GasPriceOracleArgs {
    /// Number of recent blocks to check for gas price
    #[arg(long = "gpo.blocks", default_value = "20")]
    pub blocks: Option<u32>,

    /// Gas Price below which gpo will ignore transactions
    #[arg(long = "gpo.ignoreprice", default_value = "2")]
    pub ignore_price: Option<u64>,

    /// Maximum transaction priority fee(or gasprice before London Fork) to be recommended by gpo
    #[arg(long = "gpo.maxprice", default_value = "500000000000")]
    pub max_price: Option<u64>,

    /// The percentile of gas prices to use for the estimate
    #[arg(long = "gpo.percentile", default_value = "60")]
    pub percentile: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn test_parse_gpo_args() {
        let args = CommandParser::<GasPriceOracleArgs>::parse_from(["reth"]).args;
        assert_eq!(
            args,
            GasPriceOracleArgs {
                blocks: Some(20),
                ignore_price: Some(2),
                max_price: Some(500000000000),
                percentile: Some(60),
            }
        );
    }
}
