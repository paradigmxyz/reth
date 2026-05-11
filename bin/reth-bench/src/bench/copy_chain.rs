//! Builds a target-native copy of source chain blocks and submits it through the Engine API.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::{
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{call_forkchoice_updated_with_reth, call_new_payload_with_reth},
};
use alloy_consensus::TxEnvelope;
use alloy_eips::{BlockNumberOrTag, Encodable2718};
use alloy_primitives::{Bytes, B256};
use alloy_provider::{
    network::{AnyNetwork, AnyRpcBlock},
    Provider, RootProvider,
};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload as RpcExecutionPayload,
    ExecutionPayloadEnvelopeV5, ExecutionPayloadSidecar, ForkchoiceState, JwtSecret,
    PayloadAttributes, PraguePayloadFields,
};
use alloy_transport::layers::{RateLimitRetryPolicy, RetryBackoffLayer};
use clap::Parser;
use eyre::{ensure, OptionExt, WrapErr};
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_api::ExecutionPayload as _;
use reth_node_core::args::{RpcBlockFetchRetries, WaitForPersistence};
use reth_primitives_traits::constants::GIGAGAS;
use reth_rpc_api::{RethNewPayloadInput, TestingBuildBlockRequestV1};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
use tracing::{info, warn};

const COPY_CHAIN_OUTPUT_SUFFIX: &str = "copy_chain.csv";

/// `reth-bench copy-chain` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for source block data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// First source block to copy.
    ///
    /// The target node must already be canonical at `from - 1`.
    #[arg(long, verbatim_doc_comment)]
    from: u64,

    /// Last source block to copy, inclusive.
    #[arg(long, verbatim_doc_comment)]
    to: u64,

    /// Path to a JWT secret to use for the authenticated engine-API RPC server.
    #[arg(long = "jwt-secret", alias = "jwtsecret", value_name = "PATH")]
    auth_jwtsecret: PathBuf,

    /// The RPC url to use for sending engine requests.
    #[arg(
        long,
        value_name = "ENGINE_RPC_URL",
        default_value = "http://localhost:8551",
        verbatim_doc_comment
    )]
    engine_rpc_url: String,

    /// The target node regular RPC URL used for `testing_buildBlockV1`.
    #[arg(
        long,
        value_name = "LOCAL_RPC_URL",
        default_value = "http://localhost:8545",
        verbatim_doc_comment
    )]
    local_rpc_url: String,

    /// The path to the output directory for copy manifest and benchmark results.
    #[arg(long, short, value_name = "BENCHMARK_OUTPUT", verbatim_doc_comment)]
    output: Option<PathBuf>,

    /// Optional Prometheus metrics endpoint to scrape after each copied block.
    #[arg(long = "metrics-url", value_name = "URL", verbatim_doc_comment)]
    metrics_url: Option<String>,

    /// Number of retries for fetching blocks from `--rpc-url` after a failure.
    #[arg(
        long = "rpc-block-fetch-retries",
        value_name = "RETRIES",
        default_value = "10",
        value_parser = clap::value_parser!(RpcBlockFetchRetries),
        verbatim_doc_comment
    )]
    rpc_block_fetch_retries: RpcBlockFetchRetries,

    /// Control when `reth_newPayload` waits for in-flight persistence.
    ///
    /// Accepts `always`, `never`, or a number N to wait every N blocks.
    #[arg(
        long = "wait-for-persistence",
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "always",
        default_value = "never",
        value_parser = clap::value_parser!(WaitForPersistence),
        verbatim_doc_comment
    )]
    wait_for_persistence: WaitForPersistence,

    /// Skip waiting for execution cache and sparse trie locks before processing.
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    no_wait_for_caches: bool,

    /// Skip EIP-4844 blob transactions when building the copied chain.
    ///
    /// By default, blob transactions are rejected because `testing_buildBlockV1` only accepts raw
    /// signed transactions and does not reconstruct blob sidecars.
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    skip_blob_transactions: bool,
}

#[derive(Debug, Serialize)]
struct CopyChainRow {
    source_block_number: u64,
    source_block_hash: B256,
    target_block_number: u64,
    target_block_hash: B256,
    target_parent_hash: B256,
    gas_limit: u64,
    gas_used: u64,
    transaction_count: u64,
    skipped_blob_transactions: usize,
    build_latency_us: u128,
    new_payload_latency_us: u128,
    fcu_latency_us: u128,
    total_latency_us: u128,
}

#[derive(Debug)]
struct BuiltCopyBlock {
    execution_data: ExecutionData,
    block_number: u64,
    block_hash: B256,
    parent_hash: B256,
    gas_limit: u64,
    gas_used: u64,
    transaction_count: u64,
    skipped_blob_transactions: usize,
}

impl Command {
    /// Execute `copy-chain`.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        ensure!(self.from <= self.to, "--from must be less than or equal to --to");

        if let Some(output) = &self.output {
            ensure!(!output.is_file(), "output path must be a directory");
            fs::create_dir_all(output)?;
        }

        let source_provider = block_provider(&self.rpc_url, self.rpc_block_fetch_retries).await?;
        let local_provider = RootProvider::<AnyNetwork>::new(
            ClientBuilder::default().http(self.local_rpc_url.parse()?),
        );
        let auth_provider =
            authenticated_provider(&self.engine_rpc_url, &self.auth_jwtsecret).await?;

        let target_head = local_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await?
            .ok_or_eyre("failed to fetch target latest block")?;
        ensure!(
            target_head.header.number + 1 == self.from,
            "target head is block {}, but copy range starts at {}; unwind or sync target to block {} first",
            target_head.header.number,
            self.from,
            self.from.saturating_sub(1)
        );

        info!(
            target: "reth-bench",
            from = self.from,
            to = self.to,
            target_head = target_head.header.number,
            target_hash = %target_head.header.hash,
            "Starting chain copy"
        );

        let mut metrics_scraper = MetricsScraper::maybe_new(self.metrics_url.clone());
        let total_blocks = self.to - self.from + 1;
        let total_duration = Instant::now();
        let mut target_parent_hash = target_head.header.hash;
        let mut target_hashes =
            BTreeMap::from([(target_head.header.number, target_head.header.hash)]);
        let mut rows = Vec::with_capacity(total_blocks as usize);
        let mut results = Vec::with_capacity(total_blocks as usize);

        for source_block_number in self.from..=self.to {
            let source_block = source_provider
                .get_block_by_number(source_block_number.into())
                .full()
                .await
                .wrap_err_with(|| format!("failed to fetch source block {source_block_number}"))?
                .ok_or_else(|| eyre::eyre!("source block {source_block_number} not found"))?;

            let build_start = Instant::now();
            let built = build_copy_block(
                &local_provider,
                &source_block,
                target_parent_hash,
                self.skip_blob_transactions,
            )
            .await?;
            let build_latency = build_start.elapsed();

            ensure!(
                built.block_number == source_block_number,
                "target built block number {} does not match source block {}",
                built.block_number,
                source_block_number
            );
            ensure!(
                built.parent_hash == target_parent_hash,
                "target built block parent {} does not match expected parent {}",
                built.parent_hash,
                target_parent_hash
            );

            let wait_for_persistence = self.wait_for_persistence.rpc_value(built.block_number);
            let params = serde_json::to_value((
                RethNewPayloadInput::ExecutionData(built.execution_data),
                wait_for_persistence,
                self.no_wait_for_caches.then_some(false),
            ))?;

            let new_payload_start = Instant::now();
            let server_timings = call_new_payload_with_reth(&auth_provider, None, params).await?;
            let new_payload_latency = server_timings
                .as_ref()
                .map(|timings| timings.latency)
                .unwrap_or_else(|| new_payload_start.elapsed());

            let forkchoice_state = ForkchoiceState {
                head_block_hash: built.block_hash,
                safe_block_hash: finalized_hash(&target_hashes, built.block_number, 32)
                    .unwrap_or(built.block_hash),
                finalized_block_hash: finalized_hash(&target_hashes, built.block_number, 64)
                    .unwrap_or(built.block_hash),
            };

            let fcu_start = Instant::now();
            call_forkchoice_updated_with_reth(&auth_provider, None, forkchoice_state).await?;
            let fcu_latency = fcu_start.elapsed();

            let total_latency = new_payload_latency + fcu_latency;
            let new_payload_result = NewPayloadResult {
                gas_used: built.gas_used,
                latency: new_payload_latency,
                persistence_wait: server_timings
                    .as_ref()
                    .map(|timings| timings.persistence_wait)
                    .unwrap_or_default(),
                execution_cache_wait: server_timings
                    .as_ref()
                    .map(|timings| timings.execution_cache_wait)
                    .unwrap_or_default(),
                sparse_trie_wait: server_timings
                    .as_ref()
                    .map(|timings| timings.sparse_trie_wait)
                    .unwrap_or_default(),
            };
            let combined_result = CombinedResult {
                block_number: built.block_number,
                gas_limit: built.gas_limit,
                transaction_count: built.transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            let elapsed = total_duration.elapsed();
            let gas_row = TotalGasRow {
                block_number: built.block_number,
                transaction_count: built.transaction_count,
                gas_used: built.gas_used,
                time: elapsed,
            };

            rows.push(CopyChainRow {
                source_block_number,
                source_block_hash: source_block.header.hash,
                target_block_number: built.block_number,
                target_block_hash: built.block_hash,
                target_parent_hash,
                gas_limit: built.gas_limit,
                gas_used: built.gas_used,
                transaction_count: built.transaction_count,
                skipped_blob_transactions: built.skipped_blob_transactions,
                build_latency_us: build_latency.as_micros(),
                new_payload_latency_us: new_payload_latency.as_micros(),
                fcu_latency_us: fcu_latency.as_micros(),
                total_latency_us: total_latency.as_micros(),
            });

            target_hashes.insert(built.block_number, built.block_hash);
            target_parent_hash = built.block_hash;

            if let Some(scraper) = metrics_scraper.as_mut() &&
                let Err(err) = scraper.scrape_after_block(built.block_number).await
            {
                warn!(target: "reth-bench", %err, block_number = built.block_number, "Failed to scrape metrics");
            }

            let progress = format!("{}/{}", built.block_number - self.from + 1, total_blocks);
            info!(
                target: "reth-bench",
                progress,
                build = ?build_latency,
                %combined_result,
                "Copied block"
            );

            results.push((gas_row, combined_result));
        }

        let (gas_rows, combined_results): (Vec<_>, Vec<_>) = results.into_iter().unzip();

        if let Some(path) = &self.output {
            write_copy_chain_rows(path, &rows)?;
            write_benchmark_results(path, &gas_rows, &combined_results)?;
            if let Some(scraper) = &metrics_scraper {
                scraper.write_csv(path)?;
            }
        }

        let gas_output = TotalGasOutput::with_combined_results(gas_rows, &combined_results)?;
        info!(
            target: "reth-bench",
            total_gas_used = gas_output.total_gas_used,
            total_duration = ?gas_output.total_duration,
            execution_duration = ?gas_output.execution_duration,
            blocks_processed = gas_output.blocks_processed,
            wall_clock_ggas_per_second = format_args!("{:.4}", gas_output.total_gigagas_per_second()),
            execution_ggas_per_second = format_args!("{:.4}", gas_output.execution_gigagas_per_second()),
            build_adjusted_ggas_per_second = format_args!(
                "{:.4}",
                gas_output.total_gas_used as f64 /
                    combined_results
                        .iter()
                        .map(|result| result.total_latency.as_secs_f64())
                        .sum::<f64>() /
                    GIGAGAS as f64
            ),
            "Chain copy complete"
        );

        Ok(())
    }
}

async fn block_provider(
    rpc_url: &str,
    retries: RpcBlockFetchRetries,
) -> eyre::Result<RootProvider<AnyNetwork>> {
    let retry_policy = RateLimitRetryPolicy::default().or(|_| true);
    let client = ClientBuilder::default()
        .layer(RetryBackoffLayer::new_with_policy(
            retries.as_max_retries(),
            800,
            u64::MAX,
            retry_policy,
        ))
        .http(rpc_url.parse()?);
    Ok(RootProvider::<AnyNetwork>::new(client))
}

async fn authenticated_provider(
    engine_rpc_url: &str,
    auth_jwtsecret: &Path,
) -> eyre::Result<RootProvider<AnyNetwork>> {
    let jwt = fs::read_to_string(auth_jwtsecret)
        .wrap_err_with(|| format!("failed to read JWT secret {}", auth_jwtsecret.display()))?;
    let jwt = JwtSecret::from_hex(jwt)?;
    let auth_url = Url::parse(engine_rpc_url)?;
    let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
    let client = ClientBuilder::default().connect_with(auth_transport).await?;
    Ok(RootProvider::<AnyNetwork>::new(client))
}

async fn build_copy_block(
    provider: &RootProvider<AnyNetwork>,
    source_block: &AnyRpcBlock,
    parent_hash: B256,
    skip_blob_transactions: bool,
) -> eyre::Result<BuiltCopyBlock> {
    let (request, skipped_blob_transactions) =
        build_copy_block_request(source_block, parent_hash, skip_blob_transactions)?;
    let built_payload: ExecutionPayloadEnvelopeV5 =
        provider.client().request("testing_buildBlockV1", [request]).await.wrap_err_with(|| {
            format!(
                "failed to build target-native block for source block {}",
                source_block.header.number
            )
        })?;

    let execution_data = envelope_into_execution_data(
        built_payload,
        source_block.header.parent_beacon_block_root.unwrap_or_default(),
        source_block.header.requests_hash.is_some(),
    );

    Ok(BuiltCopyBlock {
        block_number: execution_data.block_number(),
        block_hash: execution_data.block_hash(),
        parent_hash: execution_data.parent_hash(),
        gas_limit: execution_data.gas_limit(),
        gas_used: execution_data.gas_used(),
        transaction_count: execution_data.transaction_count() as u64,
        skipped_blob_transactions,
        execution_data,
    })
}

fn envelope_into_execution_data(
    envelope: ExecutionPayloadEnvelopeV5,
    parent_beacon_block_root: B256,
    include_requests: bool,
) -> ExecutionData {
    let versioned_hashes = envelope.blobs_bundle.versioned_hashes();
    let cancun_fields = CancunPayloadFields { parent_beacon_block_root, versioned_hashes };
    let sidecar = if include_requests {
        ExecutionPayloadSidecar::v4(
            cancun_fields,
            PraguePayloadFields::new(envelope.execution_requests),
        )
    } else {
        ExecutionPayloadSidecar::v3(cancun_fields)
    };

    ExecutionData { payload: RpcExecutionPayload::V3(envelope.execution_payload), sidecar }
}

fn build_copy_block_request(
    block: &AnyRpcBlock,
    parent_hash: B256,
    skip_blob_transactions: bool,
) -> eyre::Result<(TestingBuildBlockRequestV1, usize)> {
    let mut skipped_blob_transactions = 0usize;
    let transactions = block
        .clone()
        .try_into_transactions()
        .map_err(|_| eyre::eyre!("source block transactions must be fetched in full"))?
        .into_iter()
        .filter_map(|tx| {
            let tx = match TxEnvelope::try_from(tx) {
                Ok(tx) => tx,
                Err(_) => return Some(Err(eyre::eyre!("unsupported transaction type"))),
            };

            if tx.is_eip4844() {
                if !skip_blob_transactions {
                    return Some(Err(eyre::eyre!(
                        "source block {} contains an EIP-4844 blob transaction; rerun with --skip-blob-transactions or choose a range without blob transactions",
                        block.header.number
                    )))
                }
                skipped_blob_transactions += 1;
                return None
            }

            Some(Ok(Bytes::from(tx.encoded_2718())))
        })
        .collect::<eyre::Result<Vec<_>>>()?;

    let rpc_block = block.clone().into_inner();
    Ok((
        TestingBuildBlockRequestV1 {
            parent_block_hash: parent_hash,
            payload_attributes: PayloadAttributes {
                timestamp: block.header.timestamp,
                prev_randao: block.header.mix_hash.unwrap_or_default(),
                suggested_fee_recipient: block.header.beneficiary,
                withdrawals: rpc_block.withdrawals.map(|withdrawals| withdrawals.into_inner()),
                parent_beacon_block_root: block.header.parent_beacon_block_root,
                slot_number: block.header.slot_number,
            },
            transactions,
            extra_data: Some(block.header.extra_data.clone()),
        },
        skipped_blob_transactions,
    ))
}

fn finalized_hash(
    target_hashes: &BTreeMap<u64, B256>,
    block_number: u64,
    depth: u64,
) -> Option<B256> {
    target_hashes.get(&block_number.saturating_sub(depth)).copied()
}

fn write_copy_chain_rows(output_dir: &Path, rows: &[CopyChainRow]) -> eyre::Result<()> {
    fs::create_dir_all(output_dir)?;
    let output_path = output_dir.join(COPY_CHAIN_OUTPUT_SUFFIX);
    info!(target: "reth-bench", "Writing chain copy manifest to file: {:?}", output_path);
    let mut writer = csv::Writer::from_path(output_path)?;
    for row in rows {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalized_hash_uses_saturating_depth() {
        let hash = B256::repeat_byte(0x42);
        let hashes = BTreeMap::from([(0, hash)]);

        assert_eq!(finalized_hash(&hashes, 1, 64), Some(hash));
    }
}
