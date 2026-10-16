//! Integration tests for customizable gas price oracle CLI defaults.

use alloy_primitives::U256;
use clap::{Args, Parser};
use reth_node_core::args::{DefaultRpcServerArgs, GasPriceOracleArgs, RpcServerArgs};

#[derive(Parser)]
struct CommandParser<T: Args> {
    #[command(flatten)]
    args: T,
}

#[test]
fn custom_gas_price_oracle_defaults_apply_to_cli() {
    let expected = GasPriceOracleArgs {
        blocks: 7,
        ignore_price: 3,
        max_price: 1234,
        percentile: 42,
        default_suggested_fee: Some(U256::from(u64::MAX) + U256::from(1)),
    };
    DefaultRpcServerArgs::default().with_gas_price_oracle(expected).try_init().unwrap();

    assert_eq!(RpcServerArgs::default().gas_price_oracle, expected);
    assert_eq!(CommandParser::<GasPriceOracleArgs>::parse_from(["reth"]).args, expected);
    assert_eq!(
        CommandParser::<RpcServerArgs>::parse_from(["reth"]).args.gas_price_oracle,
        expected
    );

    let args = CommandParser::<RpcServerArgs>::parse_from(["reth", "--gpo.ignoreprice", "0"]).args;
    assert_eq!(args.gas_price_oracle, GasPriceOracleArgs { ignore_price: 0, ..expected });

    let args = CommandParser::<RpcServerArgs>::parse_from([
        "reth",
        "--gpo.blocks",
        "1",
        "--gpo.ignoreprice=2",
        "--gpo.maxprice",
        "100",
        "--gpo.percentile",
        "100",
        "--gpo.default-suggested-fee",
        "0",
    ])
    .args;
    assert_eq!(
        args.gas_price_oracle,
        GasPriceOracleArgs {
            blocks: 1,
            ignore_price: 2,
            max_price: 100,
            percentile: 100,
            default_suggested_fee: Some(U256::ZERO),
        }
    );
}
