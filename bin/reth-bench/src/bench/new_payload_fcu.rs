//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.
//!
//! Uses persistence-based flow: waits for every Nth block to be persisted instead of
//! using fixed wait times between blocks.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            CombinedResult, NewPayloadResult, PersistenceStats, TotalGasOutput, TotalGasRow,
            COMBINED_OUTPUT_SUFFIX, GAS_OUTPUT_SUFFIX, PERSISTENCE_STATS_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_forkchoice_updated, call_new_payload},
};
use alloy_eips::BlockNumHash;
use alloy_provider::Provider;
use alloy_rpc_types_engine::{ForkchoiceState, JwtSecret};
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use futures::{SinkExt, StreamExt};
use humantime::parse_duration;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_with_config, tungstenite::Message};
use tracing::{debug, info, warn};

/// Persistence threshold: wait for persistence every N blocks
const PERSISTENCE_THRESHOLD: u64 = 2;

/// Final sync timeout: wait up to 5 minutes for remaining blocks to persist
const PERSISTENCE_TIMEOUT: Duration = Duration::from_secs(300);

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// DEPRECATED: Wait time is ignored. Uses persistence-based flow instead.
    /// This flag is kept for backwards compatibility but has no effect.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, default_value = "0ms", hide = true, verbatim_doc_comment)]
    wait_time: Duration,

    /// The size of the block buffer (channel capacity) for prefetching blocks from the RPC
    /// endpoint.
    #[arg(
        long = "rpc-block-buffer-size",
        value_name = "RPC_BLOCK_BUFFER_SIZE",
        default_value = "20",
        verbatim_doc_comment
    )]
    rpc_block_buffer_size: usize,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        // Warn if wait_time was explicitly provided (non-zero)
        if !self.wait_time.is_zero() {
            warn!(
                "Warning: --wait-time is deprecated and ignored. \
                 Using persistence-based flow with threshold={} blocks",
                PERSISTENCE_THRESHOLD
            );
        }

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
        } = BenchContext::new(&self.benchmark, self.rpc_url).await?;

        // Set up WebSocket connection for persistence subscription
        let auth_jwt =
            self.benchmark.auth_jwtsecret.clone().ok_or_else(|| {
                eyre::eyre!("--jwt-secret must be provided for authenticated RPC")
            })?;
        let jwt_str = std::fs::read_to_string(&auth_jwt)?;
        let jwt = JwtSecret::from_hex(&jwt_str)?;

        // Convert HTTP URL to WebSocket URL for subscription
        let mut ws_url: Url = self.benchmark.engine_rpc_url.parse()?;
        match ws_url.scheme() {
            "http" => {
                ws_url.set_scheme("ws").map_err(|_| eyre::eyre!("Failed to set WS scheme"))?
            }
            "https" => {
                ws_url.set_scheme("wss").map_err(|_| eyre::eyre!("Failed to set WSS scheme"))?
            }
            "ws" | "wss" => {} // Already WebSocket
            scheme => return Err(eyre::eyre!("Unsupported URL scheme: {}", scheme)),
        }

        // Create JWT authorization header
        let claims = alloy_rpc_types_engine::Claims::default();
        let token = jwt.encode(&claims)?;
        let auth_header = format!("Bearer {}", token);

        info!("Connecting to WebSocket at {} for persistence subscription", ws_url);

        // Build WebSocket request with auth header
        let ws_request = http::Request::builder()
            .uri(ws_url.as_str())
            .header("Authorization", &auth_header)
            .header("Host", ws_url.host_str().unwrap_or("localhost"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .wrap_err("Failed to build WebSocket request")?;

        let (ws_stream, _) = connect_async_with_config(ws_request, None, false)
            .await
            .wrap_err("Failed to connect to WebSocket for persistence subscription")?;

        let (mut ws_sink, mut ws_stream) = ws_stream.split();

        // Subscribe to latest persisted block notifications via JSON-RPC
        let subscribe_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "reth_subscribeLatestPersistedBlock",
            "params": []
        });

        ws_sink
            .send(Message::Text(subscribe_request.to_string().into()))
            .await
            .wrap_err("Failed to send subscription request")?;

        // Wait for subscription confirmation
        let sub_response = ws_stream
            .next()
            .await
            .ok_or_else(|| eyre::eyre!("WebSocket closed before subscription response"))?
            .wrap_err("Failed to receive subscription response")?;

        let sub_id: String = match sub_response {
            Message::Text(text) => {
                let resp: Value = serde_json::from_str(&text)
                    .wrap_err("Failed to parse subscription response")?;
                resp["result"]
                    .as_str()
                    .ok_or_else(|| eyre::eyre!("Invalid subscription response: {}", text))?
                    .to_string()
            }
            _ => return Err(eyre::eyre!("Unexpected message type from subscription")),
        };

        info!("Subscribed to persistence notifications (subscription_id={})", sub_id);

        // Create channel for persistence notifications
        let (persistence_tx, mut persistence_rx) = mpsc::channel::<BlockNumHash>(100);

        // Spawn task to receive persistence notifications
        tokio::spawn(async move {
            while let Some(msg_result) = ws_stream.next().await {
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Ok(notification) = serde_json::from_str::<Value>(&text) {
                            // Check if this is a subscription notification
                            if notification.get("method").map(|m| m.as_str()) ==
                                Some(Some("reth_subscribeLatestPersistedBlock"))
                            {
                                if let Some(params) =
                                    notification.get("params").and_then(|p| p.get("result"))
                                {
                                    if let (Some(number), Some(hash)) = (
                                        params.get("number").and_then(|n| n.as_u64()),
                                        params.get("hash").and_then(|h| h.as_str()),
                                    ) {
                                        let block_num_hash = BlockNumHash {
                                            number,
                                            hash: hash.parse().unwrap_or_default(),
                                        };
                                        if persistence_tx.send(block_num_hash).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(e) => {
                        tracing::error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });

        info!(
            "Subscribed to persistence notifications (threshold={} blocks)",
            PERSISTENCE_THRESHOLD
        );

        let buffer_size = self.rpc_block_buffer_size;

        // Use a oneshot channel to propagate errors from the spawned task
        let (error_sender, mut error_receiver) = tokio::sync::oneshot::channel();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(buffer_size);

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
                        let _ = error_sender.send(e);
                        break;
                    }
                };

                let head_block_hash = block.header.hash;
                let safe_block_hash = block_provider
                    .get_block_by_number(block.header.number.saturating_sub(32).into());

                let finalized_block_hash = block_provider
                    .get_block_by_number(block.header.number.saturating_sub(64).into());

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
                    .send((block, head_block_hash, safe_block_hash, finalized_block_hash))
                    .await
                {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        // Persistence tracking state
        let mut blocks_sent: u64 = 0;
        let mut last_persisted_block: u64 = 0;
        let mut first_block_number: Option<u64> = None;
        let mut last_block_number: u64 = 0;

        // Persistence stats tracking
        let mut persistence_wait_count: u64 = 0;
        let mut total_persistence_wait_time = Duration::ZERO;
        let mut max_gap: u64 = 0;
        let mut gap_sum: u64 = 0;
        let mut gap_count: u64 = 0;

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((block, head, safe, finalized)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            // just put gas used here
            let gas_used = block.header.gas_used;
            let block_number = block.header.number;
            let transaction_count = block.transactions.len() as u64;

            if first_block_number.is_none() {
                first_block_number = Some(block_number);
            }
            last_block_number = block_number;

            debug!(target: "reth-bench", ?block_number, "Sending payload");

            // construct fcu to call
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let (version, params) = block_to_new_payload(block, is_optimism)?;
            let start = Instant::now();
            call_new_payload(&auth_provider, version, params).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            call_forkchoice_updated(&auth_provider, version, forkchoice_state, None).await?;

            // calculate the total duration and the fcu latency, record
            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result = CombinedResult {
                block_number,
                transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            // Track blocks sent (after newPayload, as per plan)
            blocks_sent += 1;

            // Track gap for stats
            let current_gap = block_number.saturating_sub(last_persisted_block);
            if current_gap > max_gap {
                max_gap = current_gap;
            }
            gap_sum += current_gap;
            gap_count += 1;

            // Every N blocks, wait for persistence to catch up
            if blocks_sent % PERSISTENCE_THRESHOLD == 0 {
                let target_block = block_number;
                debug!(
                    target: "reth-bench",
                    ?target_block,
                    ?last_persisted_block,
                    "Waiting for persistence"
                );

                let wait_start = Instant::now();
                while last_persisted_block < target_block {
                    match persistence_rx.recv().await {
                        Some(persisted) => {
                            last_persisted_block = persisted.number;
                            debug!(
                                target: "reth-bench",
                                persisted_block = ?last_persisted_block,
                                "Received persistence notification"
                            );
                        }
                        None => {
                            return Err(eyre::eyre!(
                                "Persistence subscription closed unexpectedly. Benchmark failed."
                            ));
                        }
                    }
                }
                let wait_elapsed = wait_start.elapsed();
                total_persistence_wait_time += wait_elapsed;
                persistence_wait_count += 1;

                debug!(
                    target: "reth-bench",
                    ?wait_elapsed,
                    "Persistence caught up"
                );
            }

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // convert gas used to gigagas, then compute gigagas per second
            info!(%combined_result);

            // record the current result
            let gas_row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        // Final sync: wait for all remaining blocks to be persisted
        if last_persisted_block < last_block_number {
            info!(
                "Waiting for final blocks to persist (current: {}, target: {})",
                last_persisted_block, last_block_number
            );

            let final_sync_result = tokio::time::timeout(PERSISTENCE_TIMEOUT, async {
                while last_persisted_block < last_block_number {
                    match persistence_rx.recv().await {
                        Some(persisted) => {
                            last_persisted_block = persisted.number;
                            debug!(
                                target: "reth-bench",
                                persisted_block = ?last_persisted_block,
                                "Final sync: received persistence notification"
                            );
                        }
                        None => {
                            return Err(eyre::eyre!("Persistence subscription closed"));
                        }
                    }
                }
                Ok(())
            })
            .await;

            match final_sync_result {
                Ok(Ok(())) => {
                    info!("All blocks persisted successfully");
                }
                Ok(Err(e)) => {
                    warn!("Final sync failed: {}", e);
                }
                Err(_) => {
                    warn!(
                        "Persistence timeout: not all blocks persisted within {:?}. \
                         Last persisted: {}, target: {}",
                        PERSISTENCE_TIMEOUT, last_persisted_block, last_block_number
                    );
                }
            }
        }

        let (gas_output_results, combined_results): (_, Vec<CombinedResult>) =
            results.into_iter().unzip();

        // Calculate persistence stats
        let persistence_stats = PersistenceStats {
            wait_count: persistence_wait_count,
            total_wait_time: total_persistence_wait_time,
            max_gap,
            avg_gap: if gap_count > 0 { gap_sum as f64 / gap_count as f64 } else { 0.0 },
        };

        // write the csv output to files
        if let Some(ref path) = self.benchmark.output {
            // first write the combined results to a file
            let output_path = path.join(COMBINED_OUTPUT_SUFFIX);
            info!("Writing engine api call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(&output_path)?;
            for result in combined_results {
                writer.serialize(result)?;
            }
            writer.flush()?;

            // now write the gas output to a file
            let output_path = path.join(GAS_OUTPUT_SUFFIX);
            info!("Writing total gas output to file: {:?}", output_path);
            let mut writer = Writer::from_path(&output_path)?;
            for row in &gas_output_results {
                writer.serialize(row)?;
            }
            writer.flush()?;

            // write persistence stats to a file
            let output_path = path.join(PERSISTENCE_STATS_SUFFIX);
            info!("Writing persistence stats to file: {:?}", output_path);
            let mut writer = Writer::from_path(&output_path)?;
            writer.serialize(&persistence_stats)?;
            writer.flush()?;

            info!("Finished writing benchmark output files to {:?}.", path);
        }

        // accumulate the results and calculate the overall Ggas/s
        let gas_output = TotalGasOutput::new(gas_output_results)?;
        info!(
            total_duration=?gas_output.total_duration,
            total_gas_used=?gas_output.total_gas_used,
            blocks_processed=?gas_output.blocks_processed,
            persistence_waits=?persistence_stats.wait_count,
            persistence_wait_time=?persistence_stats.total_wait_time,
            "Total Ggas/s: {:.4}",
            gas_output.total_gigagas_per_second()
        );

        Ok(())
    }
}
