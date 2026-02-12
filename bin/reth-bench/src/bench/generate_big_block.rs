//! Command for generating large blocks by packing transactions from real blocks.
//!
//! This command fetches transactions from existing blocks and packs them into a single
//! large block using the `testing_buildBlockV1` RPC endpoint.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect, bench::helpers::parse_gas_limit,
};
use alloy_eips::{BlockNumberOrTag, Typed2718};
use alloy_primitives::{Bytes, B256};
use alloy_provider::{ext::EngineApi, network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV4, ExecutionPayloadEnvelopeV5, ForkchoiceState, JwtSecret,
    PayloadAttributes,
};
use alloy_transport::layers::RetryBackoffLayer;
use clap::Parser;
use eyre::Context;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_rpc_api::TestingBuildBlockRequestV1;
use std::future::Future;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// A single transaction with its gas used and raw encoded bytes.
#[derive(Debug, Clone)]
pub struct RawTransaction {
    /// The actual gas used by the transaction (from receipt).
    pub gas_used: u64,
    /// The transaction type (e.g., 3 for EIP-4844 blob txs).
    pub tx_type: u8,
    /// The raw RLP-encoded transaction bytes.
    pub raw: Bytes,
}

/// Abstraction over sources of transactions for big block generation.
///
/// Implementors provide transactions from different sources (RPC, database, files, etc.)
pub trait TransactionSource {
    /// Fetch transactions from a specific block number.
    ///
    /// Returns `Ok(None)` if the block doesn't exist.
    /// Returns `Ok(Some((transactions, gas_used)))` with the block's transactions and total gas.
    fn fetch_block_transactions(
        &self,
        block_number: u64,
    ) -> impl Future<Output = eyre::Result<Option<(Vec<RawTransaction>, u64)>>> + Send;
}

/// RPC-based transaction source that fetches from a remote node.
#[derive(Debug)]
pub struct RpcTransactionSource {
    provider: RootProvider<AnyNetwork>,
}

impl RpcTransactionSource {
    /// Create a new RPC transaction source.
    pub const fn new(provider: RootProvider<AnyNetwork>) -> Self {
        Self { provider }
    }

    /// Create from an RPC URL with retry backoff.
    pub fn from_url(rpc_url: &str) -> eyre::Result<Self> {
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(rpc_url.parse()?);
        let provider = RootProvider::<AnyNetwork>::new(client);
        Ok(Self { provider })
    }
}

impl TransactionSource for RpcTransactionSource {
    async fn fetch_block_transactions(
        &self,
        block_number: u64,
    ) -> eyre::Result<Option<(Vec<RawTransaction>, u64)>> {
        // Fetch block and receipts in parallel
        let (block, receipts) = tokio::try_join!(
            self.provider.get_block_by_number(block_number.into()).full(),
            self.provider.get_block_receipts(block_number.into())
        )?;

        let Some(block) = block else {
            return Ok(None);
        };

        let Some(receipts) = receipts else {
            return Err(eyre::eyre!("Receipts not found for block {}", block_number));
        };

        let block_gas_used = block.header.gas_used;

        // Convert cumulative gas from receipts to per-tx gas_used
        let mut prev_cumulative = 0u64;
        let transactions: Vec<RawTransaction> = block
            .transactions
            .txns()
            .zip(receipts.iter())
            .map(|(tx, receipt)| {
                let cumulative = receipt.inner.inner.inner.receipt.cumulative_gas_used;
                let gas_used = cumulative - prev_cumulative;
                prev_cumulative = cumulative;

                let with_encoded = tx.inner.inner.clone().into_encoded();
                RawTransaction {
                    gas_used,
                    tx_type: tx.inner.ty(),
                    raw: with_encoded.encoded_bytes().clone(),
                }
            })
            .collect();

        Ok(Some((transactions, block_gas_used)))
    }
}

/// Collects transactions from a source up to a target gas usage.
#[derive(Debug)]
pub struct TransactionCollector<S> {
    source: S,
    target_gas: u64,
}

impl<S: TransactionSource> TransactionCollector<S> {
    /// Create a new transaction collector.
    pub const fn new(source: S, target_gas: u64) -> Self {
        Self { source, target_gas }
    }

    /// Collect transactions starting from the given block number.
    ///
    /// Skips blob transactions (type 3) and collects until target gas is reached.
    /// Returns a `CollectionResult` with transactions, gas info, and next block.
    pub async fn collect(&self, start_block: u64) -> eyre::Result<CollectionResult> {
        self.collect_gas(start_block, self.target_gas).await
    }

    /// Collect transactions up to a specific gas target.
    ///
    /// This is used both for initial collection and for retry top-ups.
    pub async fn collect_gas(
        &self,
        start_block: u64,
        gas_target: u64,
    ) -> eyre::Result<CollectionResult> {
        let mut transactions: Vec<RawTransaction> = Vec::new();
        let mut total_gas: u64 = 0;
        let mut current_block = start_block;

        while total_gas < gas_target {
            let Some((block_txs, _)) = self.source.fetch_block_transactions(current_block).await?
            else {
                warn!(target: "reth-bench", block = current_block, "Block not found, stopping");
                break;
            };

            for tx in block_txs {
                // Skip blob transactions (EIP-4844, type 3)
                if tx.tx_type == 3 {
                    continue;
                }

                if total_gas + tx.gas_used <= gas_target {
                    total_gas += tx.gas_used;
                    transactions.push(tx);
                }

                if total_gas >= gas_target {
                    break;
                }
            }

            current_block += 1;

            // Stop early if remaining gas is under 1M (close enough to target)
            let remaining_gas = gas_target.saturating_sub(total_gas);
            if remaining_gas < 1_000_000 {
                break;
            }
        }

        info!(
            target: "reth-bench",
            total_txs = transactions.len(),
            gas_sent = total_gas,
            next_block = current_block,
            "Finished collecting transactions"
        );

        Ok(CollectionResult { transactions, gas_sent: total_gas, next_block: current_block })
    }
}

/// `reth bench generate-big-block` command
///
/// Generates a large block by fetching transactions from existing blocks and packing them
/// into a single block using the `testing_buildBlockV1` RPC endpoint.
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC URL to use for fetching blocks (can be an external archive node).
    #[arg(long, value_name = "RPC_URL")]
    rpc_url: String,

    /// The engine RPC URL (with JWT authentication).
    #[arg(long, value_name = "ENGINE_RPC_URL", default_value = "http://localhost:8551")]
    engine_rpc_url: String,

    /// The RPC URL for `testing_buildBlockV1` calls (same node as engine, regular RPC port).
    #[arg(long, value_name = "TESTING_RPC_URL", default_value = "http://localhost:8545")]
    testing_rpc_url: String,

    /// Path to the JWT secret file for engine API authentication.
    #[arg(long, value_name = "JWT_SECRET")]
    jwt_secret: std::path::PathBuf,

    /// Target gas to pack into the block.
    /// Accepts short notation: K for thousand, M for million, G for billion (e.g., 1G = 1
    /// billion).
    #[arg(long, value_name = "TARGET_GAS", default_value = "30000000", value_parser = parse_gas_limit)]
    target_gas: u64,

    /// Block number to start fetching transactions from (required).
    ///
    /// This must be the last canonical block BEFORE any gas limit ramping was performed.
    /// The command collects transactions from historical blocks starting at this number
    /// to pack into large blocks.
    ///
    /// How to determine this value:
    /// - If starting from a fresh node (no gas limit ramp yet): use the current chain tip
    /// - If gas limit ramping has already been performed: use the block number that was the chain
    ///   tip BEFORE ramping began (you must track this yourself)
    ///
    /// Using a block after ramping started will cause transaction collection to fail
    /// because those blocks contain synthetic transactions that cannot be replayed.
    #[arg(long, value_name = "FROM_BLOCK")]
    from_block: u64,

    /// Execute the payload (call newPayload + forkchoiceUpdated).
    /// If false, only builds the payload and prints it.
    #[arg(long, default_value = "false")]
    execute: bool,

    /// Number of payloads to generate. Each payload uses the previous as parent.
    /// When count == 1, the payload is only generated and saved, not executed.
    /// When count > 1, each payload is executed before building the next.
    #[arg(long, default_value = "1")]
    count: u64,

    /// Number of transaction batches to prefetch in background when count > 1.
    /// Higher values reduce latency but use more memory.
    #[arg(long, default_value = "4")]
    prefetch_buffer: usize,

    /// Output directory for generated payloads. Each payload is saved as `payload_block_N.json`.
    #[arg(long, value_name = "OUTPUT_DIR")]
    output_dir: std::path::PathBuf,
}

/// A built payload ready for execution.
struct BuiltPayload {
    block_number: u64,
    envelope: ExecutionPayloadEnvelopeV4,
    block_hash: B256,
    timestamp: u64,
    /// The actual gas used in the built block.
    gas_used: u64,
}

/// Result of collecting transactions from blocks.
#[derive(Debug)]
pub struct CollectionResult {
    /// Collected transactions with their gas info.
    pub transactions: Vec<RawTransaction>,
    /// Total gas sent (sum of historical `gas_used` for all collected txs).
    pub gas_sent: u64,
    /// Next block number to continue collecting from.
    pub next_block: u64,
}

/// Constants for retry logic.
const MAX_BUILD_RETRIES: u32 = 5;
/// Maximum retries for fetching a transaction batch.
const MAX_FETCH_RETRIES: u32 = 5;
/// Tolerance: if `gas_used` is within 1M of target, don't retry.
const MIN_TARGET_SLACK: u64 = 1_000_000;
/// Maximum gas to request in retries (10x target as safety cap).
const MAX_ADDITIONAL_GAS_MULTIPLIER: u64 = 10;

/// Fetches a batch of transactions with retry logic.
///
/// Returns `None` if all retries are exhausted.
async fn fetch_batch_with_retry<S: TransactionSource>(
    collector: &TransactionCollector<S>,
    block: u64,
) -> Option<CollectionResult> {
    for attempt in 1..=MAX_FETCH_RETRIES {
        match collector.collect(block).await {
            Ok(result) => return Some(result),
            Err(e) => {
                if attempt == MAX_FETCH_RETRIES {
                    warn!(target: "reth-bench", attempt, error = %e, "Failed to fetch transactions after max retries");
                    return None;
                }
                warn!(target: "reth-bench", attempt, error = %e, "Failed to fetch transactions, retrying...");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
    None
}

/// Outcome of a build attempt check.
enum RetryOutcome {
    /// Payload is close enough to target gas.
    Success,
    /// Max retries reached, accept what we have.
    MaxRetries,
    /// Need more transactions with the specified gas amount.
    NeedMore(u64),
}

/// Buffer for receiving transaction batches from the fetcher.
///
/// This abstracts over the channel to allow the main loop to request
/// batches on demand, including for retries.
struct TxBuffer {
    receiver: mpsc::Receiver<CollectionResult>,
}

impl TxBuffer {
    const fn new(receiver: mpsc::Receiver<CollectionResult>) -> Self {
        Self { receiver }
    }

    /// Take the next available batch from the fetcher.
    async fn take_batch(&mut self) -> Option<CollectionResult> {
        self.receiver.recv().await
    }
}

impl Command {
    /// Execute the `generate-big-block` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth-bench", target_gas = self.target_gas, count = self.count, "Generating big block(s)");

        // Set up authenticated engine provider
        let jwt =
            std::fs::read_to_string(&self.jwt_secret).wrap_err("Failed to read JWT secret file")?;
        let jwt = JwtSecret::from_hex(jwt.trim())?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!(target: "reth-bench", "Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url.clone(), jwt);
        let auth_client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(auth_client);

        // Set up testing RPC provider (for testing_buildBlockV1)
        info!(target: "reth-bench", "Connecting to Testing RPC at {}", self.testing_rpc_url);
        let testing_client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(self.testing_rpc_url.parse()?);
        let testing_provider = RootProvider::<AnyNetwork>::new(testing_client);

        // Get the parent block (latest canonical block)
        info!(target: "reth-bench", endpoint = "engine", method = "eth_getBlockByNumber", block = "latest", "RPC call");
        let parent_block = auth_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        let parent_hash = parent_block.header.hash;
        let parent_number = parent_block.header.number;
        let parent_timestamp = parent_block.header.timestamp;

        info!(
            target: "reth-bench",
            parent_hash = %parent_hash,
            parent_number = parent_number,
            "Using initial parent block"
        );

        // Create output directory
        std::fs::create_dir_all(&self.output_dir).wrap_err_with(|| {
            format!("Failed to create output directory: {:?}", self.output_dir)
        })?;

        let start_block = self.from_block;

        // Use pipelined execution when generating multiple payloads
        if self.count > 1 {
            self.execute_pipelined(
                &auth_provider,
                &testing_provider,
                start_block,
                parent_hash,
                parent_timestamp,
            )
            .await?;
        } else {
            // Single payload - collect transactions and build with retry
            let tx_source = RpcTransactionSource::from_url(&self.rpc_url)?;
            let collector = TransactionCollector::new(tx_source, self.target_gas);
            let result = collector.collect(start_block).await?;

            if result.transactions.is_empty() {
                return Err(eyre::eyre!("No transactions collected"));
            }

            self.execute_sequential_with_retry(
                &auth_provider,
                &testing_provider,
                &collector,
                result,
                parent_hash,
                parent_timestamp,
            )
            .await?;
        }

        info!(target: "reth-bench", count = self.count, output_dir = %self.output_dir.display(), "All payloads generated");
        Ok(())
    }

    /// Sequential execution path with retry logic for underfilled payloads.
    async fn execute_sequential_with_retry<S: TransactionSource>(
        &self,
        auth_provider: &RootProvider<AnyNetwork>,
        testing_provider: &RootProvider<AnyNetwork>,
        collector: &TransactionCollector<S>,
        initial_result: CollectionResult,
        mut parent_hash: B256,
        mut parent_timestamp: u64,
    ) -> eyre::Result<()> {
        let mut current_result = initial_result;

        for i in 0..self.count {
            let built = self
                .build_with_retry(
                    testing_provider,
                    collector,
                    &mut current_result,
                    i,
                    parent_hash,
                    parent_timestamp,
                )
                .await?;

            self.save_payload(&built)?;

            if self.execute || self.count > 1 {
                info!(target: "reth-bench", payload = i + 1, block_hash = %built.block_hash, gas_used = built.gas_used, "Executing payload (newPayload + FCU)");
                self.execute_payload_v4(auth_provider, built.envelope, parent_hash).await?;
                info!(target: "reth-bench", payload = i + 1, "Payload executed successfully");
            }

            parent_hash = built.block_hash;
            parent_timestamp = built.timestamp;
        }
        Ok(())
    }

    /// Build a payload with retry logic when `gas_used` is below target.
    ///
    /// Uses the ratio of `gas_used/gas_sent` to estimate how many more transactions
    /// are needed to hit the target gas.
    async fn build_with_retry<S: TransactionSource>(
        &self,
        testing_provider: &RootProvider<AnyNetwork>,
        collector: &TransactionCollector<S>,
        result: &mut CollectionResult,
        index: u64,
        parent_hash: B256,
        parent_timestamp: u64,
    ) -> eyre::Result<BuiltPayload> {
        for attempt in 1..=MAX_BUILD_RETRIES {
            let tx_bytes: Vec<Bytes> = result.transactions.iter().map(|t| t.raw.clone()).collect();
            let gas_sent = result.gas_sent;

            info!(
                target: "reth-bench",
                payload = index + 1,
                attempt,
                tx_count = tx_bytes.len(),
                gas_sent,
                parent_hash = %parent_hash,
                "Building payload via testing_buildBlockV1"
            );

            let built = Self::build_payload_static(
                testing_provider,
                &tx_bytes,
                index,
                parent_hash,
                parent_timestamp,
            )
            .await?;

            match self.check_retry_outcome(&built, index, attempt, gas_sent) {
                RetryOutcome::Success | RetryOutcome::MaxRetries => return Ok(built),
                RetryOutcome::NeedMore(additional_gas) => {
                    let additional =
                        collector.collect_gas(result.next_block, additional_gas).await?;
                    result.transactions.extend(additional.transactions);
                    result.gas_sent = result.gas_sent.saturating_add(additional.gas_sent);
                    result.next_block = additional.next_block;
                }
            }
        }

        warn!(target: "reth-bench", payload = index + 1, "Retry loop exited without returning a payload");
        Err(eyre::eyre!("build_with_retry exhausted retries without result"))
    }

    /// Pipelined execution - fetches transactions in background, builds with retry.
    ///
    /// The fetcher continuously produces transaction batches. The main loop consumes them,
    /// builds payloads with retry logic (requesting more transactions if underfilled),
    /// and executes each payload before moving to the next.
    async fn execute_pipelined(
        &self,
        auth_provider: &RootProvider<AnyNetwork>,
        testing_provider: &RootProvider<AnyNetwork>,
        start_block: u64,
        initial_parent_hash: B256,
        initial_parent_timestamp: u64,
    ) -> eyre::Result<()> {
        // Create channel for transaction batches - fetcher sends CollectionResult
        let (tx_sender, tx_receiver) = mpsc::channel::<CollectionResult>(self.prefetch_buffer);

        // Spawn background task to continuously fetch transaction batches
        let rpc_url = self.rpc_url.clone();
        let target_gas = self.target_gas;

        let fetcher_handle = tokio::spawn(async move {
            let tx_source = match RpcTransactionSource::from_url(&rpc_url) {
                Ok(source) => source,
                Err(e) => {
                    warn!(target: "reth-bench", error = %e, "Failed to create transaction source");
                    return None;
                }
            };

            let collector = TransactionCollector::new(tx_source, target_gas);
            let mut current_block = start_block;

            while let Some(batch) = fetch_batch_with_retry(&collector, current_block).await {
                if batch.transactions.is_empty() {
                    info!(target: "reth-bench", block = current_block, "Reached chain tip, stopping fetcher");
                    break;
                }

                info!(
                    target: "reth-bench",
                    tx_count = batch.transactions.len(),
                    gas_sent = batch.gas_sent,
                    blocks = format!("{}..{}", current_block, batch.next_block),
                    "Fetched transaction batch"
                );
                current_block = batch.next_block;

                if tx_sender.send(batch).await.is_err() {
                    break;
                }
            }

            Some(current_block)
        });

        // Transaction buffer: holds transactions from batches + any extras from retries
        let mut tx_buffer = TxBuffer::new(tx_receiver);

        let mut parent_hash = initial_parent_hash;
        let mut parent_timestamp = initial_parent_timestamp;

        for i in 0..self.count {
            // Get initial batch of transactions for this payload
            let Some(mut result) = tx_buffer.take_batch().await else {
                info!(
                    target: "reth-bench",
                    payloads_built = i,
                    payloads_requested = self.count,
                    "Transaction source exhausted, stopping"
                );
                break;
            };

            if result.transactions.is_empty() {
                info!(
                    target: "reth-bench",
                    payloads_built = i,
                    payloads_requested = self.count,
                    "No more transactions available, stopping"
                );
                break;
            }

            // Build with retry - may need to request more transactions
            let built = self
                .build_with_retry_buffered(
                    testing_provider,
                    &mut tx_buffer,
                    &mut result,
                    i,
                    parent_hash,
                    parent_timestamp,
                )
                .await?;

            self.save_payload(&built)?;

            let current_block_hash = built.block_hash;
            let current_timestamp = built.timestamp;

            // Execute payload
            info!(target: "reth-bench", payload = i + 1, block_hash = %current_block_hash, gas_used = built.gas_used, "Executing payload (newPayload + FCU)");
            self.execute_payload_v4(auth_provider, built.envelope, parent_hash).await?;
            info!(target: "reth-bench", payload = i + 1, "Payload executed successfully");

            parent_hash = current_block_hash;
            parent_timestamp = current_timestamp;
        }

        // Clean up the fetcher task
        drop(tx_buffer);
        let _ = fetcher_handle.await;

        Ok(())
    }

    /// Build a payload with retry logic, using the buffered transaction source.
    async fn build_with_retry_buffered(
        &self,
        testing_provider: &RootProvider<AnyNetwork>,
        tx_buffer: &mut TxBuffer,
        result: &mut CollectionResult,
        index: u64,
        parent_hash: B256,
        parent_timestamp: u64,
    ) -> eyre::Result<BuiltPayload> {
        for attempt in 1..=MAX_BUILD_RETRIES {
            let tx_bytes: Vec<Bytes> = result.transactions.iter().map(|t| t.raw.clone()).collect();
            let gas_sent = result.gas_sent;

            info!(
                target: "reth-bench",
                payload = index + 1,
                attempt,
                tx_count = tx_bytes.len(),
                gas_sent,
                parent_hash = %parent_hash,
                "Building payload via testing_buildBlockV1"
            );

            let built = Self::build_payload_static(
                testing_provider,
                &tx_bytes,
                index,
                parent_hash,
                parent_timestamp,
            )
            .await?;

            match self.check_retry_outcome(&built, index, attempt, gas_sent) {
                RetryOutcome::Success | RetryOutcome::MaxRetries => return Ok(built),
                RetryOutcome::NeedMore(additional_gas) => {
                    let mut collected_gas = 0u64;
                    while collected_gas < additional_gas {
                        if let Some(batch) = tx_buffer.take_batch().await {
                            collected_gas += batch.gas_sent;
                            result.transactions.extend(batch.transactions);
                            result.gas_sent = result.gas_sent.saturating_add(batch.gas_sent);
                            result.next_block = batch.next_block;
                        } else {
                            warn!(target: "reth-bench", "Transaction fetcher exhausted, proceeding with available transactions");
                            break;
                        }
                    }
                }
            }
        }

        warn!(target: "reth-bench", payload = index + 1, "Retry loop exited without returning a payload");
        Err(eyre::eyre!("build_with_retry_buffered exhausted retries without result"))
    }

    /// Determines the outcome of a build attempt.
    fn check_retry_outcome(
        &self,
        built: &BuiltPayload,
        index: u64,
        attempt: u32,
        gas_sent: u64,
    ) -> RetryOutcome {
        let gas_used = built.gas_used;

        if gas_used + MIN_TARGET_SLACK >= self.target_gas {
            info!(
                target: "reth-bench",
                payload = index + 1,
                gas_used,
                target_gas = self.target_gas,
                attempts = attempt,
                "Payload built successfully"
            );
            return RetryOutcome::Success;
        }

        if attempt == MAX_BUILD_RETRIES {
            warn!(
                target: "reth-bench",
                payload = index + 1,
                gas_used,
                target_gas = self.target_gas,
                gas_sent,
                "Underfilled after max retries, accepting payload"
            );
            return RetryOutcome::MaxRetries;
        }

        if gas_used == 0 {
            warn!(
                target: "reth-bench",
                payload = index + 1,
                "Zero gas used in payload, requesting fixed chunk of additional transactions"
            );
            return RetryOutcome::NeedMore(self.target_gas);
        }

        let gas_sent_needed_total =
            (self.target_gas as u128 * gas_sent as u128).div_ceil(gas_used as u128) as u64;
        let additional = gas_sent_needed_total.saturating_sub(gas_sent);
        let additional = additional.min(self.target_gas * MAX_ADDITIONAL_GAS_MULTIPLIER);

        if additional == 0 {
            info!(
                target: "reth-bench",
                payload = index + 1,
                gas_used,
                target_gas = self.target_gas,
                "No additional transactions needed based on ratio"
            );
            return RetryOutcome::Success;
        }

        let ratio = gas_used as f64 / gas_sent as f64;
        info!(
            target: "reth-bench",
            payload = index + 1,
            gas_used,
            gas_sent,
            ratio = format!("{:.4}", ratio),
            additional_gas = additional,
            "Underfilled, collecting more transactions for retry"
        );
        RetryOutcome::NeedMore(additional)
    }

    /// Build a single payload via `testing_buildBlockV1`.
    async fn build_payload_static(
        testing_provider: &RootProvider<AnyNetwork>,
        transactions: &[Bytes],
        index: u64,
        parent_hash: B256,
        parent_timestamp: u64,
    ) -> eyre::Result<BuiltPayload> {
        let request = TestingBuildBlockRequestV1 {
            parent_block_hash: parent_hash,
            payload_attributes: PayloadAttributes {
                timestamp: parent_timestamp + 12,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: alloy_primitives::Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
                slot_number: None,
            },
            transactions: transactions.to_vec(),
            extra_data: None,
        };

        let total_tx_bytes: usize = transactions.iter().map(|tx| tx.len()).sum();
        info!(
            target: "reth-bench",
            payload = index + 1,
            tx_count = transactions.len(),
            total_tx_bytes = total_tx_bytes,
            parent_hash = %parent_hash,
            "Sending to testing_buildBlockV1"
        );
        let envelope: ExecutionPayloadEnvelopeV5 =
            testing_provider.client().request("testing_buildBlockV1", [request]).await?;

        let v4_envelope = envelope.try_into_v4()?;

        let inner = &v4_envelope.envelope_inner.execution_payload.payload_inner.payload_inner;
        let block_hash = inner.block_hash;
        let block_number = inner.block_number;
        let timestamp = inner.timestamp;
        let gas_used = inner.gas_used;

        Ok(BuiltPayload { block_number, envelope: v4_envelope, block_hash, timestamp, gas_used })
    }

    /// Save a payload to disk.
    fn save_payload(&self, payload: &BuiltPayload) -> eyre::Result<()> {
        let filename = format!("payload_block_{}.json", payload.block_number);
        let filepath = self.output_dir.join(&filename);
        let json = serde_json::to_string_pretty(&payload.envelope)?;
        std::fs::write(&filepath, &json)
            .wrap_err_with(|| format!("Failed to write payload to {:?}", filepath))?;
        info!(target: "reth-bench", block_number = payload.block_number, block_hash = %payload.block_hash, path = %filepath.display(), "Payload saved");
        Ok(())
    }

    async fn execute_payload_v4(
        &self,
        provider: &RootProvider<AnyNetwork>,
        envelope: ExecutionPayloadEnvelopeV4,
        parent_hash: B256,
    ) -> eyre::Result<()> {
        let block_hash =
            envelope.envelope_inner.execution_payload.payload_inner.payload_inner.block_hash;

        let status = provider
            .new_payload_v4(
                envelope.envelope_inner.execution_payload,
                vec![],
                B256::ZERO,
                envelope.execution_requests.to_vec(),
            )
            .await?;

        if !status.is_valid() {
            return Err(eyre::eyre!("Payload rejected: {:?}", status));
        }

        let fcu_state = ForkchoiceState {
            head_block_hash: block_hash,
            safe_block_hash: parent_hash,
            finalized_block_hash: parent_hash,
        };

        let fcu_result = provider.fork_choice_updated_v3(fcu_state, None).await?;

        if !fcu_result.is_valid() {
            return Err(eyre::eyre!("FCU rejected: {:?}", fcu_result));
        }

        Ok(())
    }
}
