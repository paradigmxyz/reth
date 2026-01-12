//! Command for generating large blocks by packing transactions from real blocks.
//!
//! This command fetches transactions from existing blocks and packs them into a single
//! large block using the `testing_packBlock` RPC endpoint.

use crate::authenticated_transport::AuthenticatedTransportConnect;
use alloy_consensus::Transaction;
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

/// A single transaction with its gas limit and raw encoded bytes.
#[derive(Debug, Clone)]
pub struct RawTransaction {
    /// The gas limit of the transaction.
    pub gas_limit: u64,
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
    pub fn new(provider: RootProvider<AnyNetwork>) -> Self {
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
        let block = self.provider.get_block_by_number(block_number.into()).full().await?;

        let Some(block) = block else {
            return Ok(None);
        };

        let gas_used = block.header.gas_used;
        let transactions = block
            .transactions
            .txns()
            .map(|tx| {
                let with_encoded = tx.inner.inner.clone().into_encoded();
                RawTransaction {
                    gas_limit: tx.gas_limit(),
                    tx_type: tx.inner.ty(),
                    raw: with_encoded.encoded_bytes().clone(),
                }
            })
            .collect();

        Ok(Some((transactions, gas_used)))
    }
}

/// Collects transactions from a source up to a target gas limit.
#[derive(Debug)]
pub struct TransactionCollector<S> {
    source: S,
    target_gas: u64,
}

impl<S: TransactionSource> TransactionCollector<S> {
    /// Create a new transaction collector.
    pub fn new(source: S, target_gas: u64) -> Self {
        Self { source, target_gas }
    }

    /// Collect transactions starting from the given block number.
    ///
    /// Skips blob transactions (type 3) and collects until target gas is reached.
    /// Returns the collected raw transaction bytes, total gas collected, and the next block number.
    pub async fn collect(&self, start_block: u64) -> eyre::Result<(Vec<Bytes>, u64, u64)> {
        let mut transactions: Vec<Bytes> = Vec::new();
        let mut total_gas: u64 = 0;
        let mut current_block = start_block;

        while total_gas < self.target_gas {

            let Some((block_txs, _)) =
                self.source.fetch_block_transactions(current_block).await?
            else {
                warn!(block = current_block, "Block not found, stopping");
                break;
            };

            for tx in block_txs {
                // Skip blob transactions (EIP-4844, type 3)
                if tx.tx_type == 3 {
                    continue;
                }

                if total_gas + tx.gas_limit <= self.target_gas {
                    transactions.push(tx.raw);
                    total_gas += tx.gas_limit;
                }

                if total_gas >= self.target_gas {
                    break;
                }
            }

            current_block += 1;

            // Stop early if remaining gas is under 1M (close enough to target)
            let remaining_gas = self.target_gas.saturating_sub(total_gas);
            if remaining_gas < 1_000_000 {
                break;
            }
        }

        info!(
            total_txs = transactions.len(),
            total_gas = total_gas,
            next_block = current_block,
            "Finished collecting transactions"
        );

        Ok((transactions, total_gas, current_block))
    }
}

/// `reth bench generate-big-block` command
///
/// Generates a large block by fetching transactions from existing blocks and packing them
/// into a single block using the `testing_packBlock` RPC endpoint.
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC URL to use for fetching blocks (can be an external archive node).
    #[arg(long, value_name = "RPC_URL")]
    rpc_url: String,

    /// The engine RPC URL (with JWT authentication).
    #[arg(long, value_name = "ENGINE_RPC_URL", default_value = "http://localhost:8551")]
    engine_rpc_url: String,

    /// The RPC URL for testing_packBlock calls (same node as engine, regular RPC port).
    #[arg(long, value_name = "TESTING_RPC_URL", default_value = "http://localhost:8545")]
    testing_rpc_url: String,

    /// Path to the JWT secret file for engine API authentication.
    #[arg(long, value_name = "JWT_SECRET")]
    jwt_secret: std::path::PathBuf,

    /// Target gas to pack into the block.
    #[arg(long, value_name = "TARGET_GAS", default_value = "30000000")]
    target_gas: u64,

    /// Starting block number to fetch transactions from.
    /// If not specified, starts from the engine's latest block.
    #[arg(long, value_name = "FROM_BLOCK")]
    from_block: Option<u64>,

    /// Execute the payload (call newPayload + forkchoiceUpdated).
    /// If false, only builds the payload and prints it.
    #[arg(long, default_value = "false")]
    execute: bool,

    /// Number of payloads to generate. Each payload uses the previous as parent.
    #[arg(long, default_value = "1")]
    count: u64,

    /// Number of transaction batches to prefetch in background when count > 1.
    /// Higher values reduce latency but use more memory.
    #[arg(long, default_value = "4")]
    prefetch_buffer: usize,

    /// Output directory for generated payloads. Each payload is saved as payload_NNN.json.
    #[arg(long, value_name = "OUTPUT_DIR")]
    output_dir: std::path::PathBuf,
}

/// A built payload ready for execution.
struct BuiltPayload {
    index: u64,
    envelope: ExecutionPayloadEnvelopeV4,
    block_hash: B256,
    timestamp: u64,
}

impl Command {
    /// Execute the `generate-big-block` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!(target_gas = self.target_gas, count = self.count, "Generating big block(s)");

        // Set up authenticated engine provider
        let jwt =
            std::fs::read_to_string(&self.jwt_secret).wrap_err("Failed to read JWT secret file")?;
        let jwt = JwtSecret::from_hex(jwt.trim())?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!("Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url.clone(), jwt);
        let auth_client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(auth_client);

        // Set up testing RPC provider (for testing_packBlock)
        info!("Connecting to Testing RPC at {}", self.testing_rpc_url);
        let testing_client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(self.testing_rpc_url.parse()?);
        let testing_provider = RootProvider::<AnyNetwork>::new(testing_client);

        // Get the parent block (latest canonical block)
        info!(endpoint = "engine", method = "eth_getBlockByNumber", block = "latest", "RPC call");
        let parent_block = auth_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        let parent_hash = parent_block.header.hash;
        let parent_number = parent_block.header.number;
        let parent_timestamp = parent_block.header.timestamp;

        info!(
            parent_hash = %parent_hash,
            parent_number = parent_number,
            "Using initial parent block"
        );

        // Create output directory
        std::fs::create_dir_all(&self.output_dir).wrap_err_with(|| {
            format!("Failed to create output directory: {:?}", self.output_dir)
        })?;

        let start_block = self.from_block.unwrap_or(parent_number);

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
            // Single payload - collect transactions and build
            let tx_source = RpcTransactionSource::from_url(&self.rpc_url)?;
            let collector = TransactionCollector::new(tx_source, self.target_gas);
            let (transactions, _total_gas, _next_block) = collector.collect(start_block).await?;

            if transactions.is_empty() {
                return Err(eyre::eyre!("No transactions collected"));
            }

            self.execute_sequential(
                &auth_provider,
                &testing_provider,
                transactions,
                parent_hash,
                parent_timestamp,
            )
            .await?;
        }

        info!(count = self.count, output_dir = %self.output_dir.display(), "All payloads generated");
        Ok(())
    }

    /// Sequential execution path for single payload or no-execute mode.
    async fn execute_sequential(
        &self,
        auth_provider: &RootProvider<AnyNetwork>,
        testing_provider: &RootProvider<AnyNetwork>,
        transactions: Vec<Bytes>,
        mut parent_hash: B256,
        mut parent_timestamp: u64,
    ) -> eyre::Result<()> {
        for i in 0..self.count {
            info!(
                payload = i + 1,
                total = self.count,
                parent_hash = %parent_hash,
                parent_timestamp = parent_timestamp,
                "Building payload via testing_packBlock"
            );

            let built = self
                .build_payload(testing_provider, &transactions, i, parent_hash, parent_timestamp)
                .await?;

            self.save_payload(&built)?;

            if self.execute || self.count > 1 {
                info!(payload = i + 1, block_hash = %built.block_hash, "Executing payload (newPayload + FCU)");
                self.execute_payload_v4(auth_provider, built.envelope, parent_hash).await?;
                info!(payload = i + 1, "Payload executed successfully");

                // Small delay to allow in-memory state to propagate before building next payload
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }

            parent_hash = built.block_hash;
            parent_timestamp = built.timestamp;
        }
        Ok(())
    }

    /// Pipelined execution - fetches transactions and builds payloads in background.
    async fn execute_pipelined(
        &self,
        auth_provider: &RootProvider<AnyNetwork>,
        testing_provider: &RootProvider<AnyNetwork>,
        start_block: u64,
        initial_parent_hash: B256,
        initial_parent_timestamp: u64,
    ) -> eyre::Result<()> {
        // Create channel for transaction batches (one batch per payload)
        let (tx_sender, mut tx_receiver) = mpsc::channel::<Vec<Bytes>>(self.prefetch_buffer);

        // Spawn background task to continuously fetch transaction batches
        let rpc_url = self.rpc_url.clone();
        let target_gas = self.target_gas;
        let count = self.count;

        let fetcher_handle = tokio::spawn(async move {
            let tx_source = match RpcTransactionSource::from_url(&rpc_url) {
                Ok(source) => source,
                Err(e) => {
                    warn!(error = %e, "Failed to create transaction source");
                    return;
                }
            };

            let collector = TransactionCollector::new(tx_source, target_gas);
            let mut current_block = start_block;

            for payload_idx in 0..count {
                info!(
                    payload = payload_idx + 1,
                    start_block = current_block,
                    "Fetching transactions for payload"
                );

                match collector.collect(current_block).await {
                    Ok((transactions, total_gas, next_block)) => {
                        info!(
                            payload = payload_idx + 1,
                            tx_count = transactions.len(),
                            total_gas = total_gas,
                            next_block = next_block,
                            "Fetched transactions for payload"
                        );
                        current_block = next_block;

                        if tx_sender.send(transactions).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(payload = payload_idx + 1, error = %e, "Failed to fetch transactions");
                        break;
                    }
                }
            }
        });

        let mut parent_hash = initial_parent_hash;
        let mut parent_timestamp = initial_parent_timestamp;
        let mut pending_build: Option<tokio::task::JoinHandle<eyre::Result<BuiltPayload>>> = None;

        for i in 0..self.count {
            let is_last = i == self.count - 1;

            // Get current payload (either from pending build or build now)
            let current_payload = if let Some(handle) = pending_build.take() {
                handle.await??
            } else {
                // First payload - wait for transactions and build synchronously
                let transactions = tx_receiver
                    .recv()
                    .await
                    .ok_or_else(|| eyre::eyre!("Transaction fetcher stopped unexpectedly"))?;

                if transactions.is_empty() {
                    return Err(eyre::eyre!("No transactions collected for payload {}", i + 1));
                }

                info!(
                    payload = i + 1,
                    total = self.count,
                    parent_hash = %parent_hash,
                    parent_timestamp = parent_timestamp,
                    tx_count = transactions.len(),
                    "Building payload via testing_packBlock"
                );
                self.build_payload(
                    testing_provider,
                    &transactions,
                    i,
                    parent_hash,
                    parent_timestamp,
                )
                .await?
            };

            self.save_payload(&current_payload)?;

            let current_block_hash = current_payload.block_hash;
            let current_timestamp = current_payload.timestamp;

            // Execute current payload first
            info!(payload = i + 1, block_hash = %current_block_hash, "Executing payload (newPayload + FCU)");
            self.execute_payload_v4(auth_provider, current_payload.envelope, parent_hash).await?;
            info!(payload = i + 1, "Payload executed successfully");

            // Small delay to allow in-memory state to propagate before building next payload
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            // Start building next payload in background (if not last) - AFTER execution
            if !is_last {
                // Get transactions for next payload (should already be fetched or fetching)
                let next_transactions = tx_receiver
                    .recv()
                    .await
                    .ok_or_else(|| eyre::eyre!("Transaction fetcher stopped unexpectedly"))?;

                if next_transactions.is_empty() {
                    return Err(eyre::eyre!("No transactions collected for payload {}", i + 2));
                }

                let testing_provider = testing_provider.clone();
                let next_index = i + 1;
                let total = self.count;

                pending_build = Some(tokio::spawn(async move {
                    info!(
                        payload = next_index + 1,
                        total = total,
                        parent_hash = %current_block_hash,
                        parent_timestamp = current_timestamp,
                        tx_count = next_transactions.len(),
                        "Building payload via testing_packBlock"
                    );

                    Self::build_payload_static(
                        &testing_provider,
                        &next_transactions,
                        next_index,
                        current_block_hash,
                        current_timestamp,
                    )
                    .await
                }));
            }

            parent_hash = current_block_hash;
            parent_timestamp = current_timestamp;
        }

        // Clean up the fetcher task
        drop(tx_receiver);
        let _ = fetcher_handle.await;

        Ok(())
    }

    /// Build a single payload via testing_packBlock.
    async fn build_payload(
        &self,
        testing_provider: &RootProvider<AnyNetwork>,
        transactions: &[Bytes],
        index: u64,
        parent_hash: B256,
        parent_timestamp: u64,
    ) -> eyre::Result<BuiltPayload> {
        Self::build_payload_static(
            testing_provider,
            transactions,
            index,
            parent_hash,
            parent_timestamp,
        )
        .await
    }

    /// Static version for use in spawned tasks.
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
            },
            transactions: transactions.to_vec(),
            extra_data: None,
        };

        let total_tx_bytes: usize = transactions.iter().map(|tx| tx.len()).sum();
        info!(
            payload = index + 1,
            tx_count = transactions.len(),
            total_tx_bytes = total_tx_bytes,
            parent_hash = %parent_hash,
            "Sending to testing_packBlock"
        );
        let envelope: ExecutionPayloadEnvelopeV5 =
            testing_provider.client().request("testing_packBlock", [request]).await?;

        let v4_envelope = envelope.try_into_v4()?;

        let inner = &v4_envelope.envelope_inner.execution_payload.payload_inner.payload_inner;
        let block_hash = inner.block_hash;
        let timestamp = inner.timestamp;
        let gas_used = inner.gas_used;
        let tx_count_in_block = v4_envelope.envelope_inner.execution_payload.payload_inner.payload_inner.transactions.len();

        info!(
            payload = index + 1,
            block_hash = %block_hash,
            gas_used = gas_used,
            tx_count_in_block = tx_count_in_block,
            "testing_packBlock response"
        );

        Ok(BuiltPayload { index, envelope: v4_envelope, block_hash, timestamp })
    }

    /// Save a payload to disk.
    fn save_payload(&self, payload: &BuiltPayload) -> eyre::Result<()> {
        let filename = format!("payload_{:03}.json", payload.index + 1);
        let filepath = self.output_dir.join(&filename);
        let json = serde_json::to_string_pretty(&payload.envelope)?;
        std::fs::write(&filepath, &json)
            .wrap_err_with(|| format!("Failed to write payload to {:?}", filepath))?;
        info!(payload = payload.index + 1, block_hash = %payload.block_hash, path = %filepath.display(), "Payload saved");
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

        info!(
            endpoint = "engine",
            method = "engine_newPayloadV4",
            block_hash = %block_hash,
            "RPC call"
        );

        let status = provider
            .new_payload_v4(
                envelope.envelope_inner.execution_payload,
                vec![],
                B256::ZERO,
                envelope.execution_requests.to_vec(),
            )
            .await?;

        info!(endpoint = "engine", method = "engine_newPayloadV4", ?status, "RPC response");

        if !status.is_valid() {
            return Err(eyre::eyre!("Payload rejected: {:?}", status));
        }

        let fcu_state = ForkchoiceState {
            head_block_hash: block_hash,
            safe_block_hash: parent_hash,
            finalized_block_hash: parent_hash,
        };

        info!(
            endpoint = "engine",
            method = "engine_forkchoiceUpdatedV3",
            head = %block_hash,
            safe = %parent_hash,
            finalized = %parent_hash,
            "RPC call"
        );

        let fcu_result = provider.fork_choice_updated_v3(fcu_state, None).await?;

        info!(endpoint = "engine", method = "engine_forkchoiceUpdatedV3", ?fcu_result, "RPC response");

        Ok(())
    }
}
