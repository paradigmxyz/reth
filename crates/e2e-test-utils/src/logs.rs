//! Helpers for exercising the `eth_getLogs` and poll filter RPC surface end to end.
//!
//! The helpers here cover the three things a filter test needs: a contract that emits a
//! controllable number of logs, a poller that tracks what successive `eth_getFilterChanges` calls
//! delivered, and a load generator that runs many range scans concurrently.

use crate::node::NodeTestContext;
use alloy_consensus::BlockHeader;
use alloy_network::{Ethereum, TransactionBuilder};
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockNumberOrTag, Filter, FilterId, Log, TransactionRequest};
use jsonrpsee::{core::client::ClientT, http_client::HttpClient, rpc_params};
use reth_chainspec::EthereumHardforks;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_node_builder::rpc::RethRpcAddOns;
use reth_payload_primitives::BuiltPayload;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use url::Url;

/// A deployed contract that emits `LOG1` events on demand.
///
/// The runtime code loads the log topic from the first calldata word and the number of logs to
/// emit from the second, then emits that many logs with no data. Hand written bytecode is used
/// because the e2e crates have no Solidity toolchain: the equivalent source is
///
/// ```solidity
/// fallback() external {
///     (bytes32 topic, uint256 count) = abi.decode(msg.data, (bytes32, uint256));
///     for (uint256 i = 0; i < count; i++) {
///         assembly { log1(0, 0, topic) }
///     }
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct LogEmitter {
    address: Address,
}

impl LogEmitter {
    /// Creation bytecode of the emitter contract.
    ///
    /// The first 12 bytes copy the 26 byte runtime to memory and return it, the rest is the
    /// runtime: `PUSH1 0x20 CALLDATALOAD` (count), then a loop that emits `LOG1` with the topic
    /// from calldata word 0 until the counter reaches zero.
    pub const INIT_CODE: &'static [u8] = &alloy_primitives::hex!(
        "601a600c600039601a6000f36020355b801560185760003560006000a1600190036003565b00"
    );

    /// Deploys the emitter and produces the block that contains the deployment.
    pub async fn deploy<Node, AddOns, P>(
        node: &mut NodeTestContext<Node, AddOns>,
        provider: &P,
        from: Address,
    ) -> eyre::Result<Self>
    where
        Node: FullNodeComponents<
            Types: NodeTypes<ChainSpec: EthereumHardforks>,
            Network: PeersHandleProvider,
        >,
        AddOns: RethRpcAddOns<Node>,
        P: Provider<Ethereum>,
    {
        let nonce = pending_nonce(provider, from).await?;
        let deploy = TransactionRequest::default()
            .with_from(from)
            .with_nonce(nonce)
            .with_gas_limit(1_000_000)
            .into_create()
            .with_input(Bytes::from_static(Self::INIT_CODE));

        let pending = provider.send_transaction(deploy).await?;
        node.advance_block().await?;

        let receipt = pending.get_receipt().await?;
        let address = receipt
            .contract_address
            .ok_or_else(|| eyre::eyre!("log emitter deployment did not yield an address"))?;

        Ok(Self { address })
    }

    /// The address the emitter was deployed at.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// A transaction request that emits `count` logs with `topic`.
    pub fn emit_request(&self, from: Address, topic: B256, count: u64) -> TransactionRequest {
        let mut input = Vec::with_capacity(64);
        input.extend_from_slice(topic.as_slice());
        input.extend_from_slice(&U256::from(count).to_be_bytes::<32>());

        TransactionRequest::default()
            .with_from(from)
            .with_to(self.address)
            .with_gas_limit(100_000 + count * 2_000)
            .with_input(Bytes::from(input))
    }

    /// Submits a single transaction emitting `count` logs with `topic` without producing a block.
    ///
    /// Returns the hash of the submitted transaction.
    pub async fn submit_emit<P: Provider<Ethereum>>(
        &self,
        provider: &P,
        from: Address,
        topic: B256,
        count: u64,
    ) -> eyre::Result<B256> {
        let nonce = pending_nonce(provider, from).await?;
        let request = self.emit_request(from, topic, count).with_nonce(nonce);
        Ok(*provider.send_transaction(request).await?.tx_hash())
    }

    /// Sends `txs` transactions, each emitting `logs_per_tx` logs with `topic`, and produces the
    /// block containing them.
    ///
    /// Returns the number of the produced block.
    pub async fn emit_block<Node, AddOns, P>(
        &self,
        node: &mut NodeTestContext<Node, AddOns>,
        provider: &P,
        from: Address,
        topic: B256,
        txs: u64,
        logs_per_tx: u64,
    ) -> eyre::Result<u64>
    where
        Node: FullNodeComponents<
            Types: NodeTypes<ChainSpec: EthereumHardforks>,
            Network: PeersHandleProvider,
        >,
        AddOns: RethRpcAddOns<Node>,
        P: Provider<Ethereum>,
    {
        let mut nonce = pending_nonce(provider, from).await?;
        let mut pending = Vec::with_capacity(txs as usize);
        for _ in 0..txs {
            let request = self.emit_request(from, topic, logs_per_tx).with_nonce(nonce);
            pending.push(provider.send_transaction(request).await?);
            nonce += 1;
        }

        let payload = node.advance_block().await?;

        for tx in pending {
            let receipt = tx.get_receipt().await?;
            eyre::ensure!(
                receipt.status(),
                "log emitting transaction {} reverted",
                receipt.transaction_hash
            );
            eyre::ensure!(
                receipt.logs().len() as u64 == logs_per_tx,
                "expected {logs_per_tx} logs, got {}",
                receipt.logs().len()
            );
        }

        Ok(payload.block().number())
    }
}

/// Installs a log filter and tracks what successive `eth_getFilterChanges` polls delivered.
///
/// A poll filter must hand out every log exactly once, so the poller records the identity of every
/// log it saw and remembers the ones that were delivered twice.
#[derive(Debug)]
pub struct LogFilterPoller {
    client: HttpClient,
    id: FilterId,
    seen: HashSet<(B256, u64)>,
    duplicates: Vec<Log>,
    delivered: Vec<Log>,
}

impl LogFilterPoller {
    /// Installs `filter` with `eth_newFilter` and returns a poller for it.
    pub async fn install(client: HttpClient, filter: Filter) -> eyre::Result<Self> {
        let id: FilterId = client.request("eth_newFilter", rpc_params![filter]).await?;
        Ok(Self {
            client,
            id,
            seen: HashSet::default(),
            duplicates: Vec::new(),
            delivered: Vec::new(),
        })
    }

    /// The installed filter id.
    pub const fn id(&self) -> &FilterId {
        &self.id
    }

    /// Polls the filter once and records what it returned.
    ///
    /// The RPC error is returned as is: a poll that fails must not consume the range it was
    /// covering, so tests of the error path keep polling and check that nothing was lost.
    pub async fn poll(&mut self) -> Result<Vec<Log>, jsonrpsee::core::ClientError> {
        let logs: Vec<Log> =
            self.client.request("eth_getFilterChanges", rpc_params![&self.id]).await?;

        for log in &logs {
            let identity = (log.block_hash.unwrap_or_default(), log.log_index.unwrap_or_default());
            if !self.seen.insert(identity) {
                self.duplicates.push(log.clone());
            }
        }
        self.delivered.extend(logs.iter().cloned());

        Ok(logs)
    }

    /// Every log delivered by this poller so far, in delivery order.
    pub fn delivered(&self) -> &[Log] {
        &self.delivered
    }

    /// The number of delivered logs that belong to `block`.
    pub fn delivered_in_block(&self, block: u64) -> usize {
        self.delivered.iter().filter(|log| log.block_number == Some(block)).count()
    }

    /// Fails if the same log was delivered by more than one poll.
    pub fn ensure_no_duplicates(&self) -> eyre::Result<()> {
        eyre::ensure!(
            self.duplicates.is_empty(),
            "{} logs were delivered more than once, first one is log {:?} of block {:?}",
            self.duplicates.len(),
            self.duplicates[0].log_index,
            self.duplicates[0].block_number
        );
        Ok(())
    }

    /// Calls `eth_getFilterLogs`, which always returns the filter's full range.
    pub async fn filter_logs(&self) -> eyre::Result<Vec<Log>> {
        Ok(self.client.request("eth_getFilterLogs", rpc_params![&self.id]).await?)
    }
}

/// Runs `concurrency` `eth_getLogs` range scans in parallel while sampling `eth_blockNumber`
/// latency, so a test can check that a scan storm leaves the node responsive.
///
/// Without a concurrency cap on the range scan path every scan pins a blocking pool thread, which
/// shows up as growing `eth_blockNumber` latency in the returned report.
pub async fn run_concurrent_log_scans(
    url: Url,
    filter: Filter,
    concurrency: usize,
) -> eyre::Result<LogScanReport> {
    let provider = ProviderBuilder::new().connect_http(url);
    let running = Arc::new(AtomicBool::new(true));

    let sampler = {
        let provider = provider.clone();
        let running = running.clone();
        tokio::spawn(async move {
            let mut samples = Vec::new();
            while running.load(Ordering::Relaxed) {
                let started = Instant::now();
                if provider.get_block_number().await.is_ok() {
                    samples.push(started.elapsed());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            samples
        })
    };

    let started = Instant::now();
    let scans = futures_util::future::join_all((0..concurrency).map(|_| {
        let provider = provider.clone();
        let filter = filter.clone();
        async move {
            let started = Instant::now();
            let result = provider.get_logs(&filter).await;
            (started.elapsed(), result.map(|logs| logs.len()).map_err(|err| err.to_string()))
        }
    }))
    .await;
    let elapsed = started.elapsed();

    running.store(false, Ordering::Relaxed);
    let control_latencies = sampler.await?;

    let mut durations = Vec::with_capacity(scans.len());
    let mut logs_returned = 0usize;
    let mut failures = Vec::new();
    for (duration, result) in scans {
        durations.push(duration);
        match result {
            Ok(logs) => logs_returned += logs,
            Err(err) => failures.push(err),
        }
    }

    Ok(LogScanReport { durations, logs_returned, failures, elapsed, control_latencies })
}

/// The outcome of a [`run_concurrent_log_scans`] run.
#[derive(Debug, Clone)]
pub struct LogScanReport {
    /// How long every individual scan took.
    pub durations: Vec<Duration>,
    /// Total number of logs the successful scans returned.
    pub logs_returned: usize,
    /// The errors returned by the scans that failed.
    pub failures: Vec<String>,
    /// How long it took until all scans had completed.
    pub elapsed: Duration,
    /// `eth_blockNumber` round trip times sampled while the scans were in flight.
    pub control_latencies: Vec<Duration>,
}

impl LogScanReport {
    /// The slowest `eth_blockNumber` round trip observed while the scans were running.
    ///
    /// This is the number that tells whether the scans starved the rest of the RPC surface.
    pub fn max_control_latency(&self) -> Duration {
        self.control_latencies.iter().copied().max().unwrap_or_default()
    }

    /// The slowest scan.
    pub fn max_scan_duration(&self) -> Duration {
        self.durations.iter().copied().max().unwrap_or_default()
    }

    /// Fails if any of the scans returned an error.
    pub fn ensure_all_succeeded(&self) -> eyre::Result<()> {
        eyre::ensure!(
            self.failures.is_empty(),
            "{} of {} scans failed, first error: {}",
            self.failures.len(),
            self.durations.len(),
            self.failures[0]
        );
        Ok(())
    }
}

/// Returns the next usable nonce for `from`, accounting for transactions already in the pool.
async fn pending_nonce<P: Provider<Ethereum>>(provider: &P, from: Address) -> eyre::Result<u64> {
    Ok(provider.get_transaction_count(from).block_id(BlockNumberOrTag::Pending.into()).await?)
}
