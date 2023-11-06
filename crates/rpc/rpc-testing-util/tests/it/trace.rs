use futures::StreamExt;
use jsonrpsee::http_client::HttpClientBuilder;
use reth_rpc_api_testing_util::{trace::TraceApiExt, utils::parse_env_url};
use reth_rpc_types::trace::parity::TraceType;
use std::{collections::HashSet, time::Instant};
/// This is intended to be run locally against a running node.
///
/// This is a noop of env var `RETH_RPC_TEST_NODE_URL` is not set.
#[tokio::test(flavor = "multi_thread")]
async fn trace_many_blocks() {
    let url = parse_env_url("RETH_RPC_TEST_NODE_URL");
    if url.is_err() {
        return
    }
    let url = url.unwrap();

    let client = HttpClientBuilder::default().build(url).unwrap();
    let mut stream = client.trace_block_buffered_unordered(15_000_000..=16_000_100, 20);
    let now = Instant::now();
    while let Some((err, block)) = stream.next_err().await {
        eprintln!("Error tracing block {block:?}: {err:?}");
    }
    println!("Traced all blocks in {:?}", now.elapsed());
}

/// Tests the replaying of transactions on a local Ethereum node.

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn replay_transactions() {
    let url = parse_env_url("RETH_RPC_TEST_NODE_URL").unwrap();
    let client = HttpClientBuilder::default().build(url).unwrap();

    let tx_hashes = vec![
        "0x4e08fe36db723a338e852f89f613e606b0c9a17e649b18b01251f86236a2cef3".parse().unwrap(),
        "0xea2817f1aeeb587b82f4ab87a6dbd3560fc35ed28de1be280cb40b2a24ab48bb".parse().unwrap(),
    ];

    let trace_types = HashSet::from([TraceType::StateDiff, TraceType::VmTrace]);

    let mut stream = client.replay_transactions(tx_hashes, trace_types);
    let now = Instant::now();
    while let Some(replay_txs) = stream.next().await {
        println!("Transaction: {:?}", replay_txs);
        println!("Replayed transactions in {:?}", now.elapsed());
    }
}
