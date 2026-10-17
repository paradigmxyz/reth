//! Regression coverage for configurable RPC CLI defaults.

use alloy_primitives::{map::AddressSet, Address};
use alloy_rpc_types_engine::JwtSecret;
use clap::Parser;
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
        .with_rpc_forwarder(Some("http://localhost:1234".parse().unwrap()))
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
    let mut expected = RpcServerArgs::default();
    expected.rpc_pending_block = PendingBlockKind::None;
    expected.rpc_forwarder = Some("http://localhost:5678".parse().unwrap());
    expected.rpc_send_raw_transaction_sync_timeout = Duration::from_secs(2);
    expected.rpc_jwtsecret = Some(JwtSecret::from_hex(jwt).unwrap());
    expected.builder_disallow = Some(AddressSet::default());
    expected.rpc_state_cache.max_blocks = 8;
    expected.rpc_state_cache.prewarm_bals = Some(0);
    assert_eq!(args, expected);
}
