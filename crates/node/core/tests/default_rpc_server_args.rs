//! Integration tests for customizable RPC server CLI defaults.

use clap::{Args, Parser};
use reth_node_core::args::{DefaultRpcServerArgs, RpcServerArgs};

#[derive(Parser)]
struct CommandParser<T: Args> {
    #[command(flatten)]
    args: T,
}

#[test]
fn custom_rpc_tx_fee_cap_default_applies_to_cli() {
    let fee_cap = 1000 * 10u128.pow(18);
    DefaultRpcServerArgs::default().with_rpc_tx_fee_cap(fee_cap).try_init().unwrap();

    assert_eq!(CommandParser::<RpcServerArgs>::parse_from(["reth"]).args.rpc_tx_fee_cap, fee_cap);
}
