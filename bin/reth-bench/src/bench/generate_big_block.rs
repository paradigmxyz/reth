//! Command for generating large blocks by merging transactions from consecutive real blocks.
//!
//! This command fetches consecutive blocks from an RPC until a target gas usage is reached,
//! takes block 0 as the "base" payload, concatenates transactions from subsequent blocks,
//! and saves the result to disk as a [`BigBlockPayload`] JSON file containing the merged
//! [`ExecutionData`] and environment switches at each block boundary.

use alloy_consensus::TxEnvelope;
use alloy_eips::{
    eip1559::BaseFeeParams,
    eip7840::BlobParams,
    eip7928::{AccountChanges, BlockAccessList, SlotChanges},
    Typed2718,
};
use alloy_primitives::{Bytes, B256};
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadSidecar,
    PraguePayloadFields,
};
use clap::Parser;
use eyre::Context;
use futures::{stream, StreamExt};
use reth_chainspec::EthChainSpec;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_engine_primitives::BigBlockData;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_ethereum_primitives::Receipt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
};
use tracing::{info, warn};

use crate::bench::helpers::fetch_block_access_list;

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
            .layer(alloy_transport::layers::RetryBackoffLayer::new(10, 800, u64::MAX))
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
                tracing::warn!(target: "reth-bench", block = current_block, "Block not found, stopping");
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

/// A merged big block payload with environment switches at block boundaries.
#[derive(Debug, Serialize, Deserialize)]
pub struct BigBlockPayload {
    /// The primary execution data with all concatenated transactions.
    pub execution_data: ExecutionData,
    /// Big block data containing environment switches and prior block hashes.
    #[serde(default)]
    pub big_block_data: BigBlockData<ExecutionData>,
    /// Flattened BAL across all constituent blocks, if requested during generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_access_list: Option<BlockAccessList>,
}

/// `reth bench generate-big-block` command
///
/// Generates a large block by fetching consecutive blocks from an RPC, merging their
/// transactions into a single payload, and saving the result to disk.
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC URL to use for fetching blocks.
    #[arg(long, value_name = "RPC_URL")]
    rpc_url: String,

    /// The chain name or path to a chain spec JSON file.
    #[arg(long, value_name = "CHAIN", default_value = "mainnet")]
    chain: String,

    /// Block number to start from.
    #[arg(long, value_name = "FROM_BLOCK")]
    from_block: u64,

    /// Target gas usage per big block. Consecutive real blocks are merged until
    /// this gas target is reached (or exceeded by the last included block).
    /// Accepts optional suffixes: K (thousand), M (million), G (billion).
    #[arg(long, value_name = "TARGET_GAS", value_parser = super::helpers::parse_gas_limit)]
    target_gas: u64,

    /// Number of sequential big blocks to generate.
    ///
    /// Each big block merges real blocks until `--target-gas` is reached.
    /// Sequential big blocks are chained: block N+1's `parent_hash` is set to
    /// block N's computed hash.
    #[arg(long, value_name = "NUM_BIG_BLOCKS", default_value = "1")]
    num_big_blocks: u64,

    /// Output directory for generated payloads.
    #[arg(long, value_name = "OUTPUT_DIR")]
    output_dir: std::path::PathBuf,

    /// Query `eth_getBlockAccessListByBlockNumber` for each fetched block and persist
    /// the flattened BAL on the stored payload.
    #[arg(long, default_value_t = false)]
    bal: bool,

    /// Maximum number of in-flight RPC fetches to keep buffered ahead of the merger.
    ///
    /// Each entry is one full per-block fetch (block + receipts, plus BAL when `--bal` is
    /// set). Larger values absorb RPC latency at the cost of more concurrent connections
    /// and memory; the buffer persists across `--num-big-blocks` so prefetching continues
    /// across big-block boundaries.
    #[arg(long, value_name = "PREFETCH_BUFFER", default_value_t = 32)]
    prefetch_buffer: usize,
}

impl Command {
    /// Execute the `generate-big-block` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        if self.target_gas == 0 {
            return Err(eyre::eyre!("--target-gas must be greater than 0"));
        }
        if self.num_big_blocks == 0 {
            return Err(eyre::eyre!("--num-big-blocks must be at least 1"));
        }

        // Resolve chain spec for blob params lookup
        let chain_spec = EthereumChainSpecParser::parse(&self.chain)
            .wrap_err_with(|| format!("Failed to parse chain spec: {}", self.chain))?;

        info!(
            target: "reth-bench",
            from_block = self.from_block,
            target_gas = self.target_gas,
            num_big_blocks = self.num_big_blocks,
            include_bal = self.bal,
            chain = %chain_spec.chain(),
            output_dir = %self.output_dir.display(),
            "Generating big block payloads"
        );

        // Create output directory
        std::fs::create_dir_all(&self.output_dir).wrap_err_with(|| {
            format!("Failed to create output directory: {:?}", self.output_dir)
        })?;

        // Set up RPC provider
        let client = ClientBuilder::default()
            .layer(alloy_transport::layers::RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(self.rpc_url.parse()?);
        let provider = RootProvider::<AnyNetwork>::new(client);

        let mut prev_big_block_hash: Option<B256> = None;
        let mut accumulated_block_hashes: Vec<(u64, B256)> = Vec::new();

        // Track previous big block's merged header fields for deriving basefee and
        // excess_blob_gas on subsequent big blocks.
        struct PrevBigBlockHeader {
            gas_used: u64,
            gas_limit: u64,
            base_fee_per_gas: u64,
            blob_gas_used: u64,
            excess_blob_gas: u64,
        }
        let mut prev_big_block_header: Option<PrevBigBlockHeader> = None;

        // Persistent prefetch stream: keeps `prefetch_buffer` per-block fetches in flight
        // ahead of the merger across all big blocks. Each item is a fully materialized
        // `FetchedBlock` (or `None` once the chain tip is reached on this fetch).
        let prefetch_buffer = self.prefetch_buffer.max(1);
        let bal_enabled = self.bal;
        let block_stream = stream::iter(self.from_block..)
            .map(|block_number| {
                let provider = provider.clone();
                async move { fetch_one_block(provider, block_number, bal_enabled).await }
            })
            .buffered(prefetch_buffer);
        let mut block_stream = Box::pin(block_stream);

        // Track the next block number we expect from the stream (purely for logging /
        // big-block range bookkeeping; the stream produces blocks in `from_block..` order).
        let mut next_block = self.from_block;

        for big_block_idx in 0..self.num_big_blocks {
            let range_start = next_block;

            // Drain the prefetch stream until the gas target is reached for this big block.
            let mut blocks = Vec::new();
            let mut block_access_lists: Vec<Option<BlockAccessList>> = Vec::new();
            let mut accumulated_block_gas: u64 = 0;

            let mut reached_chain_tip = false;
            while accumulated_block_gas < self.target_gas {
                let block_number = next_block;
                info!(target: "reth-bench", block_number, big_block = big_block_idx, "Awaiting prefetched block");

                let fetched = match block_stream.next().await {
                    Some(Ok(Some(fetched))) => fetched,
                    Some(Ok(None)) => {
                        warn!(
                            target: "reth-bench",
                            block_number,
                            "Block not found — reached chain tip"
                        );
                        reached_chain_tip = true;
                        break;
                    }
                    Some(Err(e)) => return Err(e),
                    // The block-number stream is open-ended; this only fires if the
                    // upstream `iter(from..)` is somehow exhausted.
                    None => {
                        reached_chain_tip = true;
                        break;
                    }
                };
                let FetchedBlock { execution_data, consensus_receipts, block_access_list } =
                    fetched;

                let block_gas = execution_data.payload.as_v1().gas_used;
                let block_blob_gas =
                    execution_data.payload.as_v3().map(|v3| v3.blob_gas_used).unwrap_or(0);

                info!(
                    target: "reth-bench",
                    block_number,
                    gas_used = block_gas,
                    blob_gas_used = block_blob_gas,
                    tx_count = execution_data.payload.transactions().len(),
                    receipts = consensus_receipts.len(),
                    "Fetched block"
                );

                accumulated_block_gas += block_gas;
                blocks.push(execution_data);
                block_access_lists.push(block_access_list);
                next_block += 1;
            }

            // If we hit the chain tip without fetching any blocks, stop generating.
            if blocks.is_empty() {
                warn!(
                    target: "reth-bench",
                    big_block = big_block_idx,
                    requested = self.num_big_blocks,
                    "No blocks available, stopping generation early"
                );
                break;
            }

            // Block 0 is the base
            let mut base = blocks.remove(0);
            let mut merged_block_access_list = block_access_lists.remove(0);
            let mut env_switches = Vec::new();

            if !blocks.is_empty() {
                // Store the original unmutated base block as env_switch at index 0.
                // This preserves the real gas_limit, basefee, etc. for segment 0's
                // EVM environment, which would otherwise be lost when we mutate the
                // base payload header below.
                env_switches.push((0, base.clone()));

                let mut cumulative_tx_count = base.payload.transactions().len();

                // Collect state from the last block for header fields
                let last = blocks.last().unwrap();
                let last_v1 = last.payload.as_v1();
                let final_state_root = last_v1.state_root;

                let mut total_gas_used = base.payload.as_v1().gas_used;
                let mut total_gas_limit = base.payload.as_v1().gas_limit;

                // Concatenate transactions from subsequent blocks and build env_switches
                for (block_idx, (block_data, block_access_list)) in
                    blocks.into_iter().zip(block_access_lists).enumerate()
                {
                    // Segment index in the merged big block. The base block is
                    // segment 0; subsequent blocks are segments 1, 2, ...
                    let segment_idx = (block_idx + 1) as u64;
                    let block_v1 = block_data.payload.as_v1();
                    let block_gas = block_v1.gas_used;
                    total_gas_used += block_gas;
                    total_gas_limit += block_v1.gas_limit;

                    if let Some(block_access_list) = block_access_list {
                        merge_block_access_list(
                            merged_block_access_list.get_or_insert_with(Default::default),
                            block_access_list,
                            cumulative_tx_count as u64,
                            segment_idx,
                        );
                    }

                    // Record environment switch at this block boundary
                    env_switches.push((cumulative_tx_count, block_data.clone()));

                    // Append this block's transactions to the base payload
                    let txs = block_data.payload.transactions().clone();
                    cumulative_tx_count += txs.len();
                    base.payload.transactions_mut().extend(txs);
                }

                // Mutate the base payload header
                let base_v1 = base.payload.as_v1_mut();
                base_v1.state_root = final_state_root;
                base_v1.gas_used = total_gas_used;
                base_v1.gas_limit = total_gas_limit;
            }

            // Chain sequential big blocks: set parent_hash, block_number, basefee,
            // and excess_blob_gas for sequential continuity. The engine validates
            // each big block against its parent, so these fields must be
            // derivable from the previous big block's merged header.
            if let Some(prev_hash) = prev_big_block_hash {
                base.payload.as_v1_mut().parent_hash = prev_hash;
                // First big block keeps its original block number (from_block).
                // Subsequent big blocks increment from there.
                base.payload.as_v1_mut().block_number = self.from_block + big_block_idx;
            }
            if let Some(prev) = &prev_big_block_header {
                // Derive basefee from the previous big block's merged header using
                // the standard EIP-1559 formula so validate_against_parent_eip1559_base_fee passes.
                let next_base_fee = alloy_eips::calc_next_block_base_fee(
                    prev.gas_used,
                    prev.gas_limit,
                    prev.base_fee_per_gas,
                    BaseFeeParams::ethereum(),
                );
                base.payload.as_v1_mut().base_fee_per_gas =
                    alloy_primitives::U256::from(next_base_fee);

                // Derive excess_blob_gas from the previous big block's merged header
                // so validate_against_parent_4844 passes.
                let timestamp = base.payload.as_v1().timestamp;
                let blob_params = chain_spec
                    .blob_params_at_timestamp(timestamp)
                    .unwrap_or_else(BlobParams::cancun);
                let next_excess_blob_gas = blob_params.next_block_excess_blob_gas_osaka(
                    prev.excess_blob_gas,
                    prev.blob_gas_used,
                    prev.base_fee_per_gas,
                );
                if let Some(v3) = base.payload.as_v3_mut() {
                    v3.excess_blob_gas = next_excess_blob_gas;
                }
            }

            // Merge blob data from all constituent blocks: sum blob_gas_used
            // and concatenate versioned hashes so the sidecar matches the blob
            // transactions in the merged payload body.
            {
                let mut all_versioned_hashes: Vec<B256> =
                    base.sidecar.cancun().map(|c| c.versioned_hashes.clone()).unwrap_or_default();
                let mut total_blob_gas =
                    base.payload.as_v3().map(|v3| v3.blob_gas_used).unwrap_or(0);
                // Skip env_switch[0] (base block clone) to avoid double-counting
                for (_, switch_data) in env_switches.iter().skip(1) {
                    if let Some(cancun) = switch_data.sidecar.cancun() {
                        all_versioned_hashes.extend_from_slice(&cancun.versioned_hashes);
                    }
                    if let Some(v3) = switch_data.payload.as_v3() {
                        total_blob_gas += v3.blob_gas_used;
                    }
                }
                if let Some(v3) = base.payload.as_v3_mut() {
                    v3.blob_gas_used = total_blob_gas;
                }
                let cancun = base.sidecar.cancun().map(|c| CancunPayloadFields {
                    versioned_hashes: all_versioned_hashes,
                    parent_beacon_block_root: c.parent_beacon_block_root,
                });
                // For merged blocks, set an empty requests hash in the Prague sidecar.
                // The correct requests_hash cannot be computed from RPC data alone
                // (raw execution layer requests are not exposed via eth_getBlockByNumber).
                // Use --testing.skip-requests-hash-check when validating big block payloads.
                let prague = base
                    .sidecar
                    .prague()
                    .map(|_| PraguePayloadFields::new(alloy_eips::eip7685::Requests::default()));
                base.sidecar = match (cancun, prague) {
                    (Some(c), Some(p)) => ExecutionPayloadSidecar::v4(c, p),
                    (Some(c), None) => ExecutionPayloadSidecar::v3(c),
                    _ => ExecutionPayloadSidecar::none(),
                };
            }

            // Compute the real block hash from the mutated payload
            let block_hash = compute_payload_block_hash(&base)?;
            base.payload.as_v1_mut().block_hash = block_hash;
            prev_big_block_hash = Some(block_hash);

            // Record this big block's merged header fields so the next big block
            // can derive its basefee and excess_blob_gas correctly.
            {
                let v1 = base.payload.as_v1();
                prev_big_block_header = Some(PrevBigBlockHeader {
                    gas_used: v1.gas_used,
                    gas_limit: v1.gas_limit,
                    base_fee_per_gas: v1.base_fee_per_gas.to::<u64>(),
                    blob_gas_used: base.payload.as_v3().map(|v3| v3.blob_gas_used).unwrap_or(0),
                    excess_blob_gas: base.payload.as_v3().map(|v3| v3.excess_blob_gas).unwrap_or(0),
                });
            }

            let big_block = BigBlockPayload {
                execution_data: base,
                big_block_data: BigBlockData {
                    env_switches,
                    prior_block_hashes: accumulated_block_hashes.clone(),
                },
                block_access_list: merged_block_access_list,
            };

            // Accumulate real block hashes from this big block's env_switches for
            // subsequent big blocks' BLOCKHASH lookups. Cap at 256 entries since the
            // BLOCKHASH opcode only looks back 256 blocks.
            for (_, switch_data) in &big_block.big_block_data.env_switches {
                let block_number = switch_data.payload.as_v1().block_number;
                let block_hash = switch_data.payload.as_v1().block_hash;
                accumulated_block_hashes.push((block_number, block_hash));
            }
            if accumulated_block_hashes.len() > 256 {
                let excess = accumulated_block_hashes.len() - 256;
                accumulated_block_hashes.drain(..excess);
            }

            // Save to disk
            let range_end = next_block - 1;
            let filename = format!("big_block_{range_start}_to_{range_end}.json");
            let filepath = self.output_dir.join(&filename);
            let json = serde_json::to_string_pretty(&big_block)?;
            std::fs::write(&filepath, &json)
                .wrap_err_with(|| format!("Failed to write payload to {:?}", filepath))?;

            info!(
                target: "reth-bench",
                path = %filepath.display(),
                block_hash = %block_hash,
                total_txs = big_block.execution_data.payload.transactions().len(),
                total_gas_used = big_block.execution_data.payload.as_v1().gas_used,
                env_switches = big_block.big_block_data.env_switches.len(),
                prior_block_hashes = big_block.big_block_data.prior_block_hashes.len(),
                bal_accounts = big_block.block_access_list.as_ref().map_or(0, Vec::len),
                "Big block payload saved"
            );

            if reached_chain_tip {
                warn!(
                    target: "reth-bench",
                    generated = big_block_idx + 1,
                    requested = self.num_big_blocks,
                    "Reached chain tip, stopping generation early"
                );
                break;
            }
        }

        Ok(())
    }
}

/// One fully-materialized block fetched by the prefetcher.
struct FetchedBlock {
    /// Execution payload with sidecar derived from the RPC block.
    execution_data: ExecutionData,
    /// Consensus-format receipts (`cumulative_gas_used` is still per-block, callers offset
    /// it when merging).
    consensus_receipts: Vec<Receipt>,
    /// `eth_getBlockAccessListByBlockNumber` result when `--bal` is enabled.
    block_access_list: Option<BlockAccessList>,
}

/// Fetches one block + receipts (and optionally its BAL) from the RPC. Returns `Ok(None)`
/// when the block doesn't exist yet (chain-tip reached).
async fn fetch_one_block(
    provider: RootProvider<AnyNetwork>,
    block_number: u64,
    bal_enabled: bool,
) -> eyre::Result<Option<FetchedBlock>> {
    let (rpc_block, receipts) = tokio::try_join!(
        provider.get_block_by_number(block_number.into()).full(),
        provider.get_block_receipts(block_number.into()),
    )?;
    let (rpc_block, receipts) = match (rpc_block, receipts) {
        (Some(b), Some(r)) => (b, r),
        _ => return Ok(None),
    };

    let block_access_list = if bal_enabled {
        Some(
            fetch_block_access_list(&provider, block_number)
                .await
                .wrap_err_with(|| format!("Failed to fetch BAL for block {block_number}"))?,
        )
    } else {
        None
    };

    let consensus_receipts: Vec<Receipt> = receipts
        .iter()
        .map(|r| {
            let inner = &r.inner.inner.inner;
            let tx_type = r.inner.inner.r#type.try_into().unwrap_or_default();
            Receipt {
                tx_type,
                success: inner.receipt.status.coerce_status(),
                cumulative_gas_used: inner.receipt.cumulative_gas_used,
                logs: inner
                    .receipt
                    .logs
                    .iter()
                    .map(|log| alloy_primitives::Log {
                        address: log.inner.address,
                        data: log.inner.data.clone(),
                    })
                    .collect(),
            }
        })
        .collect();

    let block = rpc_block
        .into_inner()
        .map_header(|header| header.map(|h| h.into_header_with_defaults()))
        .try_map_transactions(|tx| -> eyre::Result<TxEnvelope> {
            tx.try_into().map_err(|_| eyre::eyre!("unsupported tx type"))
        })?
        .into_consensus();

    let (payload, sidecar) = ExecutionPayload::from_block_slow(&block);
    let execution_data = ExecutionData { payload, sidecar };

    Ok(Some(FetchedBlock { execution_data, consensus_receipts, block_access_list }))
}

fn merge_block_access_list(
    merged: &mut BlockAccessList,
    incoming: BlockAccessList,
    tx_index_offset: u64,
    segment_idx: u64,
) {
    let mut account_positions = merged
        .iter()
        .enumerate()
        .map(|(idx, account)| (account.address, idx))
        .collect::<HashMap<_, _>>();

    for mut account_changes in incoming {
        shift_account_changes(&mut account_changes, tx_index_offset, segment_idx);

        if let Some(&idx) = account_positions.get(&account_changes.address) {
            merge_account_changes(&mut merged[idx], account_changes);
        } else {
            account_positions.insert(account_changes.address, merged.len());
            merged.push(account_changes);
        }
    }
}

fn shift_account_changes(
    account_changes: &mut AccountChanges,
    tx_index_offset: u64,
    segment_idx: u64,
) {
    // Per-block BALs use block_access_index = 0 for pre-execution writes
    // (system contract calls before any tx), 1..tx_count for tx commits, and
    // tx_count+1 for post-execution.
    //
    // Renumbering: each segment boundary reserves two distinct bal_indexes —
    // one for the prior segment's `finish()` (post-execution withdrawals +
    // EIP-7002/7251 system calls) and one for the new segment's
    // `apply_pre_execution_changes()` (EIP-2935/EIP-4788). The renumbered
    // bal_index for a block-local idx in segment `k` is
    // `idx + tx_index_offset + 2*k`. This ensures BAL workers reading via
    // `BalWrites::get` (strict less-than) see all prior segments' boundary
    // writes.
    let shift = tx_index_offset + 2 * segment_idx;
    for slot_changes in &mut account_changes.storage_changes {
        for change in &mut slot_changes.changes {
            change.block_access_index += shift;
        }
    }
    for change in &mut account_changes.balance_changes {
        change.block_access_index += shift;
    }
    for change in &mut account_changes.nonce_changes {
        change.block_access_index += shift;
    }
    for change in &mut account_changes.code_changes {
        change.block_access_index += shift;
    }
}

fn merge_account_changes(existing: &mut AccountChanges, incoming: AccountChanges) {
    merge_slot_changes(&mut existing.storage_changes, incoming.storage_changes);
    existing.storage_reads.extend(incoming.storage_reads);
    existing.balance_changes.extend(incoming.balance_changes);
    existing.nonce_changes.extend(incoming.nonce_changes);
    existing.code_changes.extend(incoming.code_changes);

    // EIP-7928 invariant: a slot must appear in either storage_changes or storage_reads,
    // not both. Per-block BALs respect this, but merging blocks can produce a slot
    // that is read in one block and changed in another. Without this normalization,
    // an empty read entry can shadow the real writes during BAL deserialization,
    // making reads of that slot fall through to stale snapshot state.
    let written: HashSet<_> =
        existing.storage_changes.iter().map(|slot_changes| slot_changes.slot).collect();
    existing.storage_reads.retain(|slot| !written.contains(slot));
    let mut seen = HashSet::with_capacity(existing.storage_reads.len());
    existing.storage_reads.retain(|slot| seen.insert(*slot));
}

fn merge_slot_changes(existing: &mut Vec<SlotChanges>, incoming: Vec<SlotChanges>) {
    let mut slot_positions = existing
        .iter()
        .enumerate()
        .map(|(idx, slot_changes)| (slot_changes.slot, idx))
        .collect::<HashMap<_, _>>();

    for slot_changes in incoming {
        if let Some(&idx) = slot_positions.get(&slot_changes.slot) {
            existing[idx].changes.extend(slot_changes.changes);
        } else {
            slot_positions.insert(slot_changes.slot, existing.len());
            existing.push(slot_changes);
        }
    }
}

/// Computes the block hash for an [`ExecutionData`] by converting it to a raw block
/// and hashing the header.
pub fn compute_payload_block_hash(data: &ExecutionData) -> eyre::Result<B256> {
    let block = data
        .payload
        .clone()
        .into_block_with_sidecar_raw(&data.sidecar)
        .wrap_err("failed to convert payload to block for hash computation")?;
    Ok(block.header.hash_slow())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip7928::{BalanceChange, CodeChange, NonceChange, StorageChange};
    use alloy_primitives::{Address, U256};

    #[test]
    fn merge_block_access_list_offsets_and_merges_accounts() {
        let shared = Address::repeat_byte(0x11);
        let other = Address::repeat_byte(0x22);

        let mut merged = vec![AccountChanges {
            address: shared,
            storage_changes: vec![SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(0, U256::from(10))],
            )],
            storage_reads: vec![U256::from(3)],
            balance_changes: vec![BalanceChange::new(1, U256::from(100))],
            nonce_changes: vec![NonceChange::new(2, 7)],
            code_changes: vec![],
        }];

        let incoming = vec![
            AccountChanges {
                address: shared,
                storage_changes: vec![
                    SlotChanges::new(U256::from(1), vec![StorageChange::new(1, U256::from(20))]),
                    SlotChanges::new(U256::from(2), vec![StorageChange::new(2, U256::from(30))]),
                ],
                storage_reads: vec![U256::from(4)],
                balance_changes: vec![BalanceChange::new(0, U256::from(150))],
                nonce_changes: vec![NonceChange::new(2, 8)],
                code_changes: vec![CodeChange::new(1, Bytes::from_static(&[0xaa]))],
            },
            AccountChanges {
                address: other,
                storage_changes: vec![SlotChanges::new(
                    U256::from(9),
                    vec![StorageChange::new(0, U256::from(90))],
                )],
                storage_reads: vec![],
                balance_changes: vec![],
                nonce_changes: vec![],
                code_changes: vec![],
            },
        ];

        merge_block_access_list(&mut merged, incoming, 3, 0);

        assert_eq!(merged.len(), 2);

        let shared = &merged[0];
        assert_eq!(shared.storage_reads, vec![U256::from(3), U256::from(4)]);
        assert_eq!(
            shared
                .balance_changes
                .iter()
                .map(|change| change.block_access_index)
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            shared.nonce_changes.iter().map(|change| change.block_access_index).collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert_eq!(shared.code_changes[0].block_access_index, 4);

        let slot_one = shared
            .storage_changes
            .iter()
            .find(|slot_changes| slot_changes.slot == U256::from(1))
            .unwrap();
        assert_eq!(
            slot_one.changes.iter().map(|change| change.block_access_index).collect::<Vec<_>>(),
            vec![0, 4]
        );

        let slot_two = shared
            .storage_changes
            .iter()
            .find(|slot_changes| slot_changes.slot == U256::from(2))
            .unwrap();
        assert_eq!(slot_two.changes[0].block_access_index, 5);

        let other = &merged[1];
        assert_eq!(other.address, Address::repeat_byte(0x22));
        assert_eq!(other.storage_changes[0].changes[0].block_access_index, 3);
    }

    #[test]
    fn merge_account_changes_normalizes_storage_reads_after_cross_block_merge() {
        let address = Address::repeat_byte(0x33);
        const A: U256 = U256::from_limbs([1, 0, 0, 0]);
        const B: U256 = U256::from_limbs([2, 0, 0, 0]);
        const C: U256 = U256::from_limbs([3, 0, 0, 0]);
        const D: U256 = U256::from_limbs([4, 0, 0, 0]);

        // Each AccountChanges value is valid on its own: storage slots only appear in
        // either reads or changes. The invalid read/change overlap is introduced when
        // these per-block BAL entries are merged for a standalone big block.
        let mut existing = AccountChanges {
            address,
            storage_changes: vec![SlotChanges::new(A, vec![StorageChange::new(0, U256::from(10))])],
            storage_reads: vec![B, C],
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        // B is read before it is written by the incoming block, and A is written before
        // it appears as a read in the incoming block. C is read in both blocks, so the
        // merge should also dedupe it. D remains read-only.
        let incoming = AccountChanges {
            address,
            storage_changes: vec![SlotChanges::new(B, vec![StorageChange::new(1, U256::from(20))])],
            storage_reads: vec![A, C, D],
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        merge_account_changes(&mut existing, incoming);

        // Written slots remain represented by storage_changes, while storage_reads only
        // keeps unique read-only slots in first-seen order.
        assert_eq!(
            existing
                .storage_changes
                .iter()
                .map(|slot_changes| slot_changes.slot)
                .collect::<Vec<_>>(),
            vec![A, B]
        );
        assert_eq!(existing.storage_reads, vec![C, D]);
        assert!(existing.storage_reads.iter().all(|read_slot| {
            !existing.storage_changes.iter().any(|slot_changes| slot_changes.slot == *read_slot)
        }));
    }
}
