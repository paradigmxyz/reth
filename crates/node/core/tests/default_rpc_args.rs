//! Regression coverage for configurable RPC CLI defaults.

use alloy_primitives::{map::AddressSet, Address};
use alloy_rpc_types_engine::JwtSecret;
use clap::{CommandFactory, Parser};
use reth_node_core::args::{DefaultRpcServerArgs, RpcServerArgs, RpcStateCacheArgs};
use reth_rpc_eth_types::builder::config::PendingBlockKind;
use std::time::Duration;

#[derive(Parser)]
struct CommandParser {
    #[command(flatten)]
    args: RpcServerArgs,
}

#[test]
fn custom_rpc_defaults_apply_to_cli() {
    DefaultRpcServerArgs::default()
        .with_rpc_pending_block(PendingBlockKind::Empty)
        .with_rpc_forwarder(Some(
            "https://user:pass%3Aword@localhost:1234/path-token?key=query-token".parse().unwrap(),
        ))
        .with_rpc_send_raw_transaction_sync_timeout(Duration::from_millis(250))
        .with_rpc_jwtsecret(Some(JwtSecret::from_hex("11".repeat(32)).unwrap()))
        .with_builder_disallow(Some(AddressSet::from_iter([Address::repeat_byte(1)])))
        .with_rpc_state_cache(RpcStateCacheArgs {
            max_blocks: 1,
            max_receipts: 2,
            max_headers: 3,
            max_bals: 4,
            max_blocks_bytes: 1024,
            max_receipts_bytes: 2048,
            max_bals_bytes: 4096,
            idle_timeout: Duration::from_millis(500),
            cache_computed_bals: true,
            prewarm_bals: Some(5),
            max_concurrent_db_requests: 6,
            max_cached_tx_hashes: 7,
        })
        .try_init()
        .unwrap();

    assert_eq!(CommandParser::parse_from(["reth"]).args, RpcServerArgs::default());

    let mut command = CommandParser::command();
    let mut without_forwarder_default =
        CommandParser::command().mut_arg("rpc_forwarder", |arg| arg.default_value(None::<&str>));
    assert_eq!(
        command.render_help().to_string(),
        without_forwarder_default.render_help().to_string()
    );
    assert_eq!(
        command.render_long_help().to_string(),
        without_forwarder_default.render_long_help().to_string()
    );

    for flag in ["--rpc.forwarder", "--rpc-forwarder"] {
        let error = CommandParser::try_parse_from(["reth", flag, "configured"]).err().unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    let jwt = "22".repeat(32);
    let args = CommandParser::parse_from([
        "reth",
        "--rpc.pending-block",
        "none",
        "--rpc.forwarder",
        "http://localhost:5678",
        "--rpc.send-raw-transaction-sync-timeout",
        "2s",
        "--rpc.jwtsecret",
        &jwt,
        "--builder.disallow",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/disallowed-addresses.json"),
        "--rpc-cache.max-blocks",
        "8",
        "--rpc-cache.prewarm-bals",
    ])
    .args;
    let mut expected = RpcServerArgs {
        rpc_pending_block: PendingBlockKind::None,
        rpc_forwarder: Some("http://localhost:5678".parse().unwrap()),
        rpc_send_raw_transaction_sync_timeout: Duration::from_secs(2),
        rpc_jwtsecret: Some(JwtSecret::from_hex(jwt).unwrap()),
        builder_disallow: Some(AddressSet::default()),
        ..Default::default()
    };
    expected.rpc_state_cache.max_blocks = 8;
    expected.rpc_state_cache.prewarm_bals = Some(0);
    assert_eq!(args, expected);
}
