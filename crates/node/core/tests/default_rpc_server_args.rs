//! Integration tests for customizable RPC server CLI defaults.

use clap::{Args, CommandFactory, Parser};
use reth_node_core::args::{DefaultRpcServerArgs, RpcServerArgs};

#[derive(Parser)]
struct CommandParser<T: Args> {
    #[command(flatten)]
    args: T,
}

#[test]
fn custom_rpc_tx_fee_cap_default_applies_to_cli_and_default() {
    let fee_cap = 1000 * 10u128.pow(18);
    DefaultRpcServerArgs::default().with_rpc_tx_fee_cap(fee_cap).try_init().unwrap();

    assert_eq!(CommandParser::<RpcServerArgs>::parse_from(["reth"]).args.rpc_tx_fee_cap, fee_cap);
    assert_eq!(RpcServerArgs::default().rpc_tx_fee_cap, fee_cap);

    let help = CommandParser::<RpcServerArgs>::command().render_long_help().to_string();
    let fee_cap_help = help.split("--rpc.txfeecap").nth(1).unwrap();
    let fee_cap_help = fee_cap_help.split("--").next().unwrap();
    assert!(fee_cap_help.contains("[default: 1000]"), "{fee_cap_help}");
}
