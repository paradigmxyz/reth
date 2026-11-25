//! Integration tests for customizable RPC server CLI defaults.

use clap::{Args, CommandFactory, FromArgMatches, Parser};
use reth_node_core::args::{DefaultRpcServerArgs, RpcServerArgs};
use reth_rpc_server_types::{RethRpcModule, RpcModuleSelection};

#[derive(Parser)]
struct CommandParser<T: Args> {
    #[command(flatten)]
    args: T,
}

#[test]
fn custom_rpc_server_defaults_apply_to_cli() {
    let fee_cap = 1000 * 10u128.pow(18);
    let http_api =
        RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3]);
    DefaultRpcServerArgs::default()
        .with_rpc_tx_fee_cap(fee_cap)
        .with_http_api(Some(http_api.clone()))
        .with_ws_api(Some(RpcModuleSelection::Standard))
        .try_init()
        .unwrap();

    let args = CommandParser::<RpcServerArgs>::parse_from(["reth"]).args;
    assert_eq!(args.rpc_tx_fee_cap, fee_cap);
    assert_eq!(args.http_api, Some(http_api));
    assert_eq!(args.ws_api, Some(RpcModuleSelection::Standard));

    // A command-level default still takes precedence over the global one.
    let matches = CommandParser::<RpcServerArgs>::command()
        .mut_arg("http_api", |arg| arg.default_value("admin"))
        .get_matches_from(["reth"]);
    let args = CommandParser::<RpcServerArgs>::from_arg_matches(&matches).unwrap().args;
    assert_eq!(args.http_api, Some(RpcModuleSelection::from([RethRpcModule::Admin])));
}
