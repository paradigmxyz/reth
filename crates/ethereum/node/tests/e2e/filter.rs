//! End-to-end coverage for `eth_getLogs` and the poll filter API.

use crate::utils::eth_payload_attributes;
use alloy_primitives::{b256, Address, B256};
use alloy_provider::{network::EthereumWallet, Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, FilterId, Log};
use jsonrpsee::{
    core::client::{ClientT, Error as ClientError},
    http_client::HttpClient,
    rpc_params,
    types::error::INVALID_PARAMS_CODE,
};
use reth_chainspec::{ChainSpec, ChainSpecBuilder, MAINNET};
use reth_e2e_test_utils::{
    logs::{run_concurrent_log_scans, LogEmitter, LogFilterPoller},
    E2ETestSetupBuilder, NodeHelperType,
};
use reth_node_core::node_config::NodeConfig;
use reth_node_ethereum::EthereumNode;
use reth_provider::{BlockIdReader, ReceiptProvider};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Topic of every log the test contract emits.
const TOPIC: B256 = b256!("0x00000000000000000000000000000000000000000000000000000000000000aa");

#[tokio::test]
async fn test_get_filter_changes_delivers_each_log_once() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|config| config).await?;
    let client = node.rpc_client().unwrap();

    // a block with logs that predates the filter
    let before_install = emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 2).await?;

    // a watch filter installed with an explicit fromBlock, the pattern that rescans on every poll
    let filter = Filter::new().from_block(0u64).address(emitter.address());
    let mut poller = LogFilterPoller::install(client, filter).await?;
    poller.poll().await?;

    let mut produced = Vec::new();
    for _ in 0..3 {
        produced.push(emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 2).await?);
        poller.poll().await?;
    }

    poller.ensure_no_duplicates()?;
    for number in produced {
        assert_eq!(
            poller.delivered_in_block(number),
            2,
            "the two logs of block {number} must be delivered exactly once"
        );
    }

    // eth_getFilterLogs keeps returning the full range the filter covers
    let all = poller.filter_logs().await?;
    assert_eq!(all.len(), 8);
    assert!(
        all.iter().any(|log| log.block_number == Some(before_install)),
        "eth_getFilterLogs must still return logs from before the filter was installed"
    );

    Ok(())
}

#[tokio::test]
async fn test_get_filter_changes_with_future_to_block() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|config| config).await?;
    let client = node.rpc_client().unwrap();

    let to_block = provider.get_block_number().await? + 2;
    let filter = Filter::new().from_block(0u64).to_block(to_block).address(emitter.address());
    let mut poller = LogFilterPoller::install(client, filter).await?;

    // a filter whose toBlock is still ahead of the chain must poll, not error
    poller.poll().await?;

    let mut produced = Vec::new();
    for _ in 0..4 {
        produced.push(emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 1).await?);
        poller.poll().await?;
    }

    poller.ensure_no_duplicates()?;
    for number in produced {
        let expected = usize::from(number <= to_block);
        assert_eq!(
            poller.delivered_in_block(number),
            expected,
            "block {number} is {} the filter's toBlock {to_block}",
            if number <= to_block { "within" } else { "beyond" }
        );
    }

    let exhausted = poller.poll().await?;
    assert!(exhausted.is_empty(), "a filter past its toBlock must stay empty, got {exhausted:?}");

    Ok(())
}

#[tokio::test]
async fn test_get_filter_changes_keeps_range_when_log_limit_hit() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|mut config| {
        config.rpc.rpc_max_logs_per_response = 3u64.into();
        config
    })
    .await?;
    let client = node.rpc_client().unwrap();

    let filter = Filter::new().address(emitter.address());
    let mut poller = LogFilterPoller::install(client, filter).await?;

    let mut produced = Vec::new();
    for _ in 0..4 {
        produced.push(emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 2).await?);
    }

    // the pending range holds more logs than a single response may carry, so the poll has to page.
    // whichever way it does that, no poll may drop the range it was covering
    let mut errors = Vec::new();
    for _ in 0..10 {
        if poller.delivered().len() >= 8 {
            break
        }
        if let Err(err) = poller.poll().await {
            errors.push(err.to_string());
        }
    }

    poller.ensure_no_duplicates()?;
    assert_eq!(
        poller.delivered().len(),
        8,
        "polling must deliver every log of the pending range, poll errors: {errors:?}"
    );
    for number in produced {
        assert_eq!(poller.delivered_in_block(number), 2, "block {number} was delivered partially");
    }

    Ok(())
}

#[tokio::test]
async fn test_pending_transaction_filter_polls_within_one_block() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|config| config).await?;
    let client = node.rpc_client().unwrap();

    let id: FilterId = client.request("eth_newPendingTransactionFilter", rpc_params![]).await?;

    let first_tx = emitter.submit_emit(&provider, from, TOPIC, 1).await?;
    let first: Vec<B256> = client.request("eth_getFilterChanges", rpc_params![&id]).await?;
    assert!(first.contains(&first_tx), "first poll must return the pending transaction");

    // no block was produced in between, the filter still has to report the new transaction
    let second_tx = emitter.submit_emit(&provider, from, TOPIC, 1).await?;
    let second: Vec<B256> = client.request("eth_getFilterChanges", rpc_params![&id]).await?;
    assert!(
        second.contains(&second_tx),
        "a pending transaction filter must not be gated on head progress"
    );
    assert!(!second.contains(&first_tx), "a poll must not repeat what the previous one delivered");

    // the filter survives being polled on a chain that does not advance
    node.advance_block().await?;
    let third: Vec<B256> = client.request("eth_getFilterChanges", rpc_params![&id]).await?;
    assert!(third.is_empty(), "no transactions were submitted, got {third:?}");

    Ok(())
}

#[tokio::test]
async fn test_get_logs_pending_to_block_is_stable() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|config| config).await?;
    let client = node.rpc_client().unwrap();

    emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 2).await?;

    let filter = json!({
        "fromBlock": "0x0",
        "toBlock": "pending",
        "address": emitter.address(),
    });
    let without_pending: Vec<Log> =
        client.request("eth_getLogs", rpc_params![filter.clone()]).await?;
    assert_eq!(without_pending.len(), 2);

    // a payload that has not been made canonical yet is the node's pending block
    emitter.submit_emit(&provider, from, TOPIC, 1).await?;
    let pending = node.build_and_submit_payload().await?;
    assert_eq!(
        pending.block().body().transactions().count(),
        1,
        "the pending block must carry the emitted log for this to prove anything"
    );
    assert!(
        node.inner.provider.pending_block_num_hash()?.is_some(),
        "expected the submitted payload to become the pending block"
    );

    let with_pending: Vec<Log> = client.request("eth_getLogs", rpc_params![filter]).await?;
    assert_eq!(
        with_pending, without_pending,
        "a pending toBlock must not depend on whether the engine has a pending block"
    );

    Ok(())
}

#[tokio::test]
async fn test_get_logs_query_limits() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|mut config| {
        config.rpc.rpc_max_blocks_per_filter = 3u64.into();
        config.rpc.rpc_max_logs_per_response = 3u64.into();
        config
    })
    .await?;
    let client = node.rpc_client().unwrap();

    let mut blocks = Vec::new();
    for _ in 0..4 {
        blocks.push(emitter.emit_block(&mut node, &provider, from, TOPIC, 1, 4).await?);
    }

    // a range wider than the block limit is rejected with the configured limit
    let error = get_logs_error(&client, log_range(&emitter, 0, blocks[3])).await;
    assert_eq!(error.code(), INVALID_PARAMS_CODE);
    assert!(
        error.message().contains("query exceeds max block range 3"),
        "unexpected message: {}",
        error.message()
    );

    // a range with more logs than fit into one response names the range to retry with
    let error = get_logs_error(&client, log_range(&emitter, blocks[0], blocks[1])).await;
    assert_eq!(error.code(), INVALID_PARAMS_CODE);
    assert!(
        error.message().contains(&format!("retry with the range {}-{}", blocks[0], blocks[0])),
        "the retry range must end at the last block that fit, got: {}",
        error.message()
    );

    // the suggested range is a single block, which is exempt from the log limit
    let retried: Vec<Log> = client
        .request("eth_getLogs", rpc_params![log_range(&emitter, blocks[0], blocks[0])])
        .await?;
    assert_eq!(
        retried.len(),
        4,
        "a single block query must return all of its logs, even above the log limit"
    );

    Ok(())
}

#[tokio::test]
async fn test_get_logs_over_pruned_receipts() -> eyre::Result<()> {
    // receipts are only pruned once the tip is `PruneSegment::Receipts::min_blocks()` (64) past
    // the prune target, and static file pruning drops whole jars, so the chain has to be long
    // enough for both
    const PRUNE_BEFORE: u64 = 5;
    const CHAIN_LENGTH: u64 = 80;

    let (mut node, provider, emitter, from) = setup(|mut config| {
        config.static_files.blocks_per_file_receipts = Some(2);
        config.pruning.receipts_before = Some(PRUNE_BEFORE);
        // the rpc receipt cache outlives pruning, so keep it too small to answer from
        config.rpc.rpc_state_cache.max_receipts = 1;
        config
    })
    .await?;
    let client = node.rpc_client().unwrap();

    // logs that end up in the range that gets pruned. the receipts are deliberately not read back
    // over rpc, that would populate the cache the pruned read has to miss
    emitter.submit_emit(&provider, from, TOPIC, 2).await?;
    let pruned_block = node.advance_block().await?.block().number;
    assert!(pruned_block < PRUNE_BEFORE);
    assert_eq!(
        node.inner
            .provider
            .receipts_by_block(pruned_block.into())?
            .map(|receipts| receipts.iter().map(|receipt| receipt.logs.len()).sum::<usize>()),
        Some(2),
        "the block to be pruned must hold the emitted logs"
    );

    while provider.get_block_number().await? < CHAIN_LENGTH {
        node.advance_block().await?;
    }

    wait_for_pruned_receipts(&node, pruned_block).await?;

    // the logs of the pruned range are gone, which the node must report instead of answering with
    // a silently incomplete result
    let error = get_logs_error(&client, log_range(&emitter, 0, CHAIN_LENGTH)).await;
    assert_eq!(error.code(), INVALID_PARAMS_CODE);
    assert!(
        error.message().contains("pruned"),
        "a query over pruned receipts must fail, got: {}",
        error.message()
    );

    // the range whose receipts are retained is still served
    let retained: Vec<Log> = client
        .request("eth_getLogs", rpc_params![log_range(&emitter, PRUNE_BEFORE, CHAIN_LENGTH)])
        .await?;
    assert!(retained.is_empty());

    Ok(())
}

#[tokio::test]
#[ignore = "measures responsiveness under a scan storm, asserts nothing until eth_getLogs has a concurrency cap (#26999)"]
async fn test_get_logs_concurrent_scan_load() -> eyre::Result<()> {
    let (mut node, provider, emitter, from) = setup(|mut config| {
        config.rpc.rpc_max_logs_per_response = 0u64.into();
        config
    })
    .await?;

    for _ in 0..20 {
        emitter.emit_block(&mut node, &provider, from, TOPIC, 2, 25).await?;
    }

    let filter = Filter::new().from_block(0u64).to_block(BlockNumberOrTag::Latest);
    let report = run_concurrent_log_scans(node.rpc_url(), filter, 64).await?;
    report.ensure_all_succeeded()?;

    println!(
        "64 concurrent full range scans: {:?} total, slowest scan {:?}, {} logs, \
         slowest eth_blockNumber while scanning {:?} over {} samples",
        report.elapsed,
        report.max_scan_duration(),
        report.logs_returned,
        report.max_control_latency(),
        report.control_latencies.len(),
    );

    Ok(())
}

/// Launches a single node with the given config tweaks and deploys the log emitting contract.
async fn setup(
    modifier: impl Fn(NodeConfig<ChainSpec>) -> NodeConfig<ChainSpec> + Send + Sync + 'static,
) -> eyre::Result<(NodeHelperType<EthereumNode>, impl Provider + Clone, LogEmitter, Address)> {
    reth_tracing::init_test_tracing();

    let chain_spec = Arc::new(
        ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(serde_json::from_str(include_str!("../assets/genesis.json")).unwrap())
            .cancun_activated()
            .build(),
    );

    let (mut nodes, wallet) =
        E2ETestSetupBuilder::<EthereumNode, _>::new(1, chain_spec, eth_payload_attributes)
            .with_node_config_modifier(modifier)
            .build()
            .await?;

    let mut node = nodes.pop().unwrap();
    let signer = wallet.wallet_gen().swap_remove(0);
    let from = signer.address();
    let provider =
        ProviderBuilder::new().wallet(EthereumWallet::new(signer)).connect_http(node.rpc_url());
    let emitter = LogEmitter::deploy(&mut node, &provider, from).await?;

    Ok((node, provider, emitter, from))
}

/// An `eth_getLogs` filter for the emitter's logs in the given inclusive block range.
fn log_range(emitter: &LogEmitter, from: u64, to: u64) -> serde_json::Value {
    json!({
        "fromBlock": format!("0x{from:x}"),
        "toBlock": format!("0x{to:x}"),
        "address": emitter.address(),
    })
}

/// Waits until the pruner has removed the receipts of `block`.
async fn wait_for_pruned_receipts(
    node: &NodeHelperType<EthereumNode>,
    block: u64,
) -> eyre::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while node.inner.provider.receipts_by_block(block.into())?.is_some() {
        eyre::ensure!(
            Instant::now() < deadline,
            "pruner did not remove the receipts of block {block}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Runs `eth_getLogs` and returns the error it failed with.
async fn get_logs_error(
    client: &HttpClient,
    filter: serde_json::Value,
) -> jsonrpsee::types::ErrorObjectOwned {
    let error = client
        .request::<Vec<Log>, _>("eth_getLogs", rpc_params![filter])
        .await
        .expect_err("query should have been rejected");
    let ClientError::Call(error) = error else { panic!("expected a call error, got {error:?}") };
    error
}
