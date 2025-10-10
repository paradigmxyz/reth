//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow, COMBINED_OUTPUT_SUFFIX,
            GAS_OUTPUT_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_forkchoice_updated, call_new_payload},
};
use alloy_eips::Encodable2718;
use alloy_primitives::{hex, Bytes};
use alloy_provider::Provider;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_node_api::EngineApiMessageVersion;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

const REORG_HEIGHT: usize = 8;

/// Structure to hold a built block with metadata
#[derive(Debug, Clone)]
struct BuiltBlock {
    block_number: u64,
    #[allow(dead_code)]
    payload: serde_json::Value,
    block_hash: alloy_primitives::B256,
    #[allow(dead_code)]
    tx_count: usize,
    #[allow(dead_code)]
    timestamp: u64,
}

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// How long to wait after a forkchoice update before sending the next payload.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, verbatim_doc_comment)]
    wait_time: Option<Duration>,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
        } = BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider
                    .get_block_by_number(next_block.into())
                    .full()
                    .await
                    .wrap_err_with(|| format!("Failed to fetch block by number {next_block}"));
                let block = match block_res.and_then(|opt| opt.ok_or_eyre("Block not found")) {
                    Ok(block) => block,
                    Err(e) => {
                        tracing::error!("Failed to fetch block {next_block}: {e}");
                        break;
                    }
                };
                let header = block.header.clone();
                let transactions = block.transactions.clone();

                let (version, params) = match block_to_new_payload(block, is_optimism) {
                    Ok(result) => result,
                    Err(e) => {
                        tracing::error!("Failed to convert block to new payload: {e}");
                        break;
                    }
                };
                let head_block_hash = header.hash;
                let safe_block_hash =
                    block_provider.get_block_by_number(header.number.saturating_sub(32).into());

                let finalized_block_hash =
                    block_provider.get_block_by_number(header.number.saturating_sub(64).into());

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash,);

                let safe_block_hash = match safe {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                let finalized_block_hash = match finalized {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                next_block += 1;
                if let Err(e) = sender
                    .send((
                        header,
                        version,
                        params,
                        head_block_hash,
                        safe_block_hash,
                        finalized_block_hash,
                        transactions,
                    ))
                    .await
                {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        // List to track built blocks, clears when reaching REORG_HEIGHT
        let mut built_blocks_list: Vec<BuiltBlock> = Vec::with_capacity(REORG_HEIGHT);

        while let Some((header, version, params, head, safe, finalized, transactions)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            // just put gas used here
            let gas_used = header.gas_used;
            let block_number = header.number;

            info!(target: "reth-bench", ?block_number, "Sending transactions to pool",);

            // Send all transactions to the transaction pool first
            // The transactions field contains full transactions when using .full()
            if transactions.is_full() {
                // Extract the full transactions from the enum
                let txs = transactions.as_transactions().unwrap_or_default();
                for tx in txs {
                    // Encode the transaction to raw bytes
                    let mut encoded = Vec::new();
                    tx.inner.inner.encode_2718(&mut encoded);
                    let raw_tx = Bytes::from(encoded);

                    // Send raw transaction to the pool via RPC
                    let _: alloy_primitives::TxHash = auth_provider
                        .client()
                        .request("eth_sendRawTransaction", (raw_tx,))
                        .await
                        .unwrap_or_else(|e| {
                            debug!(target: "reth-bench", "Failed to send transaction: {e}");
                            // Return a dummy hash on error to continue processing
                            alloy_primitives::TxHash::default()
                        });
                }
            } else {
                warn!(target: "reth-bench", ?block_number, "Block has transaction hashes only, skipping tx pool submission");
            }

            // After sending transactions, trigger block building with forkchoiceUpdate
            info!(target: "reth-bench", ?block_number, "Triggering block building with forkchoiceUpdate");

            // Clear the list if it has reached capacity
            if built_blocks_list.len() >= REORG_HEIGHT {
                info!(
                    target: "reth-bench",
                    list_size = built_blocks_list.len(),
                    "List reached REORG_HEIGHT capacity, clearing list before building"
                );
                built_blocks_list.clear();
            }

            // Determine parent block for building:
            // - If we have blocks in the list, use the most recent one as parent
            // - Otherwise, fall back to using the parent of the RPC block
            let building_parent_hash = if let Some(latest_built_block) = built_blocks_list.last()
            {
                // Use the most recent built block from our list as parent
                info!(
                    target: "reth-bench",
                    ?block_number,
                    built_parent = ?latest_built_block.block_hash,
                    built_parent_number = latest_built_block.block_number,
                    "Using most recent built block from list as parent for new build"
                );
                latest_built_block.block_hash
            } else {
                // List is empty, use the parent from RPC block
                info!(
                    target: "reth-bench",
                    ?block_number,
                    rpc_parent = ?header.parent_hash,
                    "List is empty, using RPC block's parent for building"
                );
                header.parent_hash
            };

            // Create payload attributes to trigger block building
            // Note: We still use timestamp and other attributes from the RPC block
            let timestamp = header.timestamp;
            let prev_randao = header.mix_hash.unwrap_or_default();
            let suggested_fee_recipient = header.beneficiary;

            // Check if the block has withdrawals (post-Shanghai)
            // If the current block has withdrawals, we need to include them in payload attributes
            let withdrawals = header.withdrawals_root.is_some().then(Vec::new);

            // Check for parent beacon block root (post-Cancun)
            let parent_beacon_block_root = header.parent_beacon_block_root;

            let payload_attributes = PayloadAttributes {
                timestamp,
                prev_randao,
                suggested_fee_recipient,
                withdrawals,
                parent_beacon_block_root,
            };

            // Call forkchoiceUpdate with payload attributes to trigger building
            let fcu_for_building = ForkchoiceState {
                head_block_hash: building_parent_hash, // Use determined parent as head for building
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            // Call forkchoiceUpdate to start building
            let fcu_result = call_forkchoice_updated(
                &auth_provider,
                version,
                fcu_for_building,
                Some(payload_attributes),
            )
            .await?;

            // Extract payload ID if building was triggered
            if let Some(payload_id) = fcu_result.payload_id {
                info!(target: "reth-bench", ?block_number, ?payload_id, "Block building triggered, retrieving built block");

                // Call getPayload to retrieve the built block
                let method = match version {
                    reth_node_api::EngineApiMessageVersion::V1 => "engine_getPayloadV1",
                    reth_node_api::EngineApiMessageVersion::V2 => "engine_getPayloadV2",
                    reth_node_api::EngineApiMessageVersion::V3 |
                    reth_node_api::EngineApiMessageVersion::V4 |
                    reth_node_api::EngineApiMessageVersion::V5 => "engine_getPayloadV3",
                };

                // Retrieve the built payload
                let built_payload: serde_json::Value =
                    auth_provider.client().request(method, (payload_id,)).await.unwrap_or_else(
                        |e| {
                            warn!(target: "reth-bench", "Failed to get built payload: {e}");
                            serde_json::Value::Null
                        },
                    );

                if !built_payload.is_null() {
                    // Extract information from the built block
                    let built_block_hash_str = built_payload
                        .get("blockHash")
                        .or_else(|| {
                            built_payload.get("executionPayload").and_then(|p| p.get("blockHash"))
                        })
                        .and_then(|h| h.as_str())
                        .unwrap_or(
                            "0x0000000000000000000000000000000000000000000000000000000000000000",
                        );

                    // Parse the hash to B256
                    let built_block_hash = built_block_hash_str.parse::<alloy_primitives::B256>()
                        .unwrap_or_else(|_| {
                            warn!(target: "reth-bench", "Failed to parse built block hash, using zero hash");
                            alloy_primitives::B256::ZERO
                        });

                    let built_tx_count = built_payload
                        .get("transactions")
                        .or_else(|| {
                            built_payload
                                .get("executionPayload")
                                .and_then(|p| p.get("transactions"))
                        })
                        .and_then(|txs| txs.as_array())
                        .map(|txs| txs.len())
                        .unwrap_or(0);

                    // Extract timestamp from the built block
                    let built_timestamp = built_payload
                        .get("timestamp")
                        .or_else(|| {
                            built_payload.get("executionPayload").and_then(|p| p.get("timestamp"))
                        })
                        .and_then(|t| t.as_u64())
                        .unwrap_or(timestamp);

                    // IMPORTANT: Import the built block into the chain via newPayload
                    // This is necessary so the block exists in the chain before we can use it as a
                    // parent
                    info!(target: "reth-bench", ?block_number, ?built_block_hash, "Importing built block via newPayload");

                    // Extract the execution payload for newPayload call
                    let execution_payload = if built_payload.get("executionPayload").is_some() {
                        // V2+ format - extract the executionPayload field
                        built_payload.get("executionPayload").unwrap()
                    } else {
                        // V1 format - the response IS the execution payload
                        &built_payload
                    };

                    // Prepare parameters based on version - the params should match what was used
                    // for original block
                    let built_params = match version {
                        EngineApiMessageVersion::V1 => {
                            // V1 just takes the execution payload
                            serde_json::json!([execution_payload])
                        }
                        EngineApiMessageVersion::V2 => {
                            // V2 also just takes the execution payload
                            serde_json::json!([execution_payload])
                        }
                        EngineApiMessageVersion::V3 |
                        EngineApiMessageVersion::V4 |
                        EngineApiMessageVersion::V5 => {
                            // V3+ need versioned hashes calculated from the built block's blob
                            // transactions We need to extract blob
                            // versioned hashes from the actual transactions in the built block

                            // Calculate versioned hashes from the built block's transactions
                            let built_versioned_hashes = if let Some(txs) =
                                execution_payload.get("transactions").and_then(|t| t.as_array())
                            {
                                // Parse each transaction to check if it's a blob transaction (type
                                // 0x03)
                                let versioned_hashes: Vec<serde_json::Value> = Vec::new();
                                for tx_bytes in txs {
                                    if let Some(tx_str) = tx_bytes.as_str() {
                                        // Remove 0x prefix and decode hex
                                        let tx_hex = tx_str.trim_start_matches("0x");
                                        if let Ok(tx_bytes) = hex::decode(tx_hex) {
                                            // Check if this is a blob transaction (first byte is
                                            // 0x03)
                                            if !tx_bytes.is_empty() && tx_bytes[0] == 0x03 {
                                                // For blob transactions, we would need to parse and
                                                // extract
                                                // versioned hashes For now, we'll
                                                // skip blob transactions in the built block
                                                // This is a simplification - in production you'd
                                                // parse the blob tx
                                            }
                                        }
                                    }
                                }
                                serde_json::json!(versioned_hashes)
                            } else {
                                // No transactions or couldn't parse them
                                serde_json::json!([])
                            };

                            // Get parent beacon block root from original params
                            let original_params_array = params.as_array().unwrap();
                            let parent_beacon_block_root =
                                original_params_array.get(2).unwrap_or(&serde_json::Value::Null);

                            // For V4+, there might be a 4th parameter (requests/execution requests)
                            if original_params_array.len() > 3 {
                                let requests = original_params_array.get(3).unwrap();
                                serde_json::json!([
                                    execution_payload,
                                    built_versioned_hashes,
                                    parent_beacon_block_root,
                                    requests
                                ])
                            } else {
                                serde_json::json!([
                                    execution_payload,
                                    built_versioned_hashes,
                                    parent_beacon_block_root
                                ])
                            }
                        }
                    };

                    // Call newPayload to import the built block
                    match call_new_payload(&auth_provider, version, built_params).await {
                        Ok(_) => {
                            info!(target: "reth-bench", ?block_number, ?built_block_hash, "Successfully imported built block");
                        }
                        Err(e) => {
                            warn!(target: "reth-bench", ?block_number, ?built_block_hash, "Failed to import built block: {e}");
                            // Continue anyway - the block might not be valid but we'll still track
                            // it
                        }
                    }

                    // Create BuiltBlock struct
                    let built_block = BuiltBlock {
                        block_number,
                        payload: built_payload.clone(),
                        block_hash: built_block_hash,
                        tx_count: built_tx_count,
                        timestamp: built_timestamp,
                    };

                    // Add to list
                    built_blocks_list.push(built_block);

                    // Count original transactions
                    let original_tx_count = if transactions.is_full() {
                        transactions.as_transactions().map(|txs| txs.len()).unwrap_or(0)
                    } else {
                        0
                    };

                    // Log the comparison and list status
                    info!(
                        target: "reth-bench",
                        original_block_hash = ?head,
                        built_block_hash = ?built_block_hash,
                        original_tx_count = original_tx_count,
                        built_tx_count = built_tx_count,
                        list_size = built_blocks_list.len(),
                        "Block building comparison - Added to list"
                    );
                }
            } else {
                warn!(target: "reth-bench", ?block_number, "No payload ID returned from forkchoiceUpdate");
            }

            debug!(target: "reth-bench", ?block_number, "Sending payload",);

            // construct fcu to call for the actual block
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let start = Instant::now();
            call_new_payload(&auth_provider, version, params.clone()).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            // No longer sending built blocks via newPayload - they're stored in the list

            call_forkchoice_updated(&auth_provider, version, forkchoice_state, None).await?;

            // calculate the total duration and the fcu latency, record
            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result =
                CombinedResult { block_number, new_payload_result, fcu_latency, total_latency };

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // convert gas used to gigagas, then compute gigagas per second
            info!(%combined_result);

            // wait if we need to
            if let Some(wait_time) = self.wait_time {
                tokio::time::sleep(wait_time).await;
            }

            // record the current result
            let gas_row = TotalGasRow { block_number, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
        }

        let (gas_output_results, combined_results): (_, Vec<CombinedResult>) =
            results.into_iter().unzip();

        // write the csv output to files
        if let Some(path) = self.benchmark.output {
            // first write the combined results to a file
            let output_path = path.join(COMBINED_OUTPUT_SUFFIX);
            info!("Writing engine api call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for result in combined_results {
                writer.serialize(result)?;
            }
            writer.flush()?;

            // now write the gas output to a file
            let output_path = path.join(GAS_OUTPUT_SUFFIX);
            info!("Writing total gas output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for row in &gas_output_results {
                writer.serialize(row)?;
            }
            writer.flush()?;

            info!("Finished writing benchmark output files to {:?}.", path);
        }

        // accumulate the results and calculate the overall Ggas/s
        let gas_output = TotalGasOutput::new(gas_output_results)?;
        info!(
            total_duration=?gas_output.total_duration,
            total_gas_used=?gas_output.total_gas_used,
            blocks_processed=?gas_output.blocks_processed,
            "Total Ggas/s: {:.4}",
            gas_output.total_gigagas_per_second()
        );

        Ok(())
    }
}
