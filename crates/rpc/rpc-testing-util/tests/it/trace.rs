use jsonrpsee::http_client::HttpClientBuilder;
use reth_rpc_api_testing_util::{trace::TraceApiExt, utils::parse_env_url};
use std::time::Instant;

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
