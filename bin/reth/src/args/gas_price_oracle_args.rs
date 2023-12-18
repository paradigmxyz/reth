use crate::primitives::U256;
use clap::Args;
use reth_rpc::eth::gas_oracle::GasPriceOracleConfig;
use reth_rpc_builder::constants::{
    DEFAULT_GAS_PRICE_BLOCKS, DEFAULT_GAS_PRICE_PERCENTILE, DEFAULT_IGNORE_GAS_PRICE,
    DEFAULT_MAX_GAS_PRICE,
};

/// Parameters to configure Gas Price Oracle
#[derive(Debug, Clone, Copy, Args, PartialEq, Eq)]
#[clap(next_help_heading = "Gas Price Oracle")]
pub struct GasPriceOracleArgs {
    /// Number of recent blocks to check for gas price
    #[arg(long = "gpo.blocks", default_value_t = DEFAULT_GAS_PRICE_BLOCKS)]
    pub blocks: u32,

    /// Gas Price below which gpo will ignore transactions
    #[arg(long = "gpo.ignoreprice", default_value_t = DEFAULT_IGNORE_GAS_PRICE.to())]
    pub ignore_price: u64,

    /// Maximum transaction priority fee(or gasprice before London Fork) to be recommended by gpo
    #[arg(long = "gpo.maxprice", default_value_t = DEFAULT_MAX_GAS_PRICE.to())]
    pub max_price: u64,

    /// The percentile of gas prices to use for the estimate
    #[arg(long = "gpo.percentile", default_value_t = DEFAULT_GAS_PRICE_PERCENTILE)]
    pub percentile: u32,
}

impl GasPriceOracleArgs {
    /// Returns a [GasPriceOracleConfig] from the arguments.
    pub fn gas_price_oracle_config(&self) -> GasPriceOracleConfig {
        let Self { blocks, ignore_price, max_price, percentile } = self;
        GasPriceOracleConfig {
            max_price: Some(U256::from(*max_price)),
            ignore_price: Some(U256::from(*ignore_price)),
            percentile: *percentile,
            blocks: *blocks,
            ..Default::default()
        }
    }
}

impl Default for GasPriceOracleArgs {
    fn default() -> Self {
        Self {
            blocks: DEFAULT_GAS_PRICE_BLOCKS,
            ignore_price: DEFAULT_IGNORE_GAS_PRICE.to(),
            max_price: DEFAULT_MAX_GAS_PRICE.to(),
            percentile: DEFAULT_GAS_PRICE_PERCENTILE,
        }
    }
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
                blocks: DEFAULT_GAS_PRICE_BLOCKS,
                ignore_price: DEFAULT_IGNORE_GAS_PRICE.to(),
                max_price: DEFAULT_MAX_GAS_PRICE.to(),
                percentile: DEFAULT_GAS_PRICE_PERCENTILE,
            }
        );
    }

    #[test]
    fn gpo_args_default_sanity_test() {
        let default_args = GasPriceOracleArgs::default();
        let args = CommandParser::<GasPriceOracleArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
