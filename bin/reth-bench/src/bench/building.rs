//! Runs the `reth benchmark` command, calling first forkchoiceUpdated for each block, then calling
//! getPayload, then newPayload.

use std::time::{Duration, Instant};

use crate::{
    authenticated_transport::AuthenticatedTransportConnect, bench_mode::BenchMode,
    valid_payload::EngineApiValidWaitExt,
};
use alloy_consensus::TxEnvelope;
use alloy_eips::eip2718::Encodable2718;
use alloy_provider::{
    ext::EngineApi, network::AnyNetwork, Provider, ProviderBuilder, RootProvider,
};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ForkchoiceState, JwtSecret, PayloadAttributes};
use clap::Parser;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::{Block, B256};
use reth_rpc_types::BlockNumberOrTag;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;
use tracing::{debug, info};

/// `reth benchmark building` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Running benchmark using data from RPC URL: {}", self.rpc_url);

        // set up alloy client for blocks
        let block_provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);

        // If neither `--from` nor `--to` are provided, we will run the benchmark continuously,
        // starting at the latest block.
        let mut benchmark_mode = match (self.benchmark.from, self.benchmark.to) {
            (Some(from), Some(to)) => BenchMode::Range(from..=to),
            (None, None) => BenchMode::Continuous,
            _ => {
                // both or neither are allowed, everything else is ambiguous
                return Err(eyre::eyre!(
                    "Both --benchmark.from and --benchmark.to must be provided together"
                ))
            }
        };

        // construct the authenticated provider
        let auth_jwt = self.benchmark.auth_jwtsecret.ok_or_else(|| {
            eyre::eyre!("--auth-jwtsecret must be provided for authenticated RPC")
        })?;

        // fetch jwt from file
        //
        // the jwt is hex encoded so we will decode it after
        let jwt = std::fs::read_to_string(auth_jwt)?;
        let jwt = JwtSecret::from_hex(jwt)?;

        // get engine url
        let auth_url = Url::parse(&self.benchmark.engine_rpc_url)?;

        // construct the authed transport
        info!("Connecting to Engine RPC at {} for replay", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_boxed(auth_transport).await?;
        let auth_provider = RootProvider::<_, AnyNetwork>::new(client);

        let first_block = match benchmark_mode {
            BenchMode::Continuous => {
                // fetch Latest block
                block_provider.get_block_by_number(BlockNumberOrTag::Latest, true).await?.unwrap()
            }
            BenchMode::Range(ref mut range) => {
                match range.next() {
                    Some(block_number) => {
                        // fetch first block in range
                        block_provider
                            .get_block_by_number(block_number.into(), true)
                            .await?
                            .unwrap()
                    }
                    None => {
                        // return an error
                        panic!("RangeEmpty");
                    }
                }
            }
        };

        let mut next_block = match first_block.header.number {
            Some(number) => {
                // fetch next block
                number + 1
            }
            None => {
                // this should never happen
                // TODO: log or return error, we should probably not return the
                // block here
                panic!("BlockNumberNone");
            }
        };

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider.get_block_by_number(next_block.into(), true).await;
                let block = block_res.unwrap().unwrap();

                let payload_attributes = PayloadAttributes {
                    prev_randao: block.header.mix_hash.unwrap(),
                    parent_beacon_block_root: block.header.parent_beacon_block_root,
                    suggested_fee_recipient: block.header.miner,
                    withdrawals: block.withdrawals,
                    timestamp: block.header.timestamp,
                };

                let head_block_hash = block.header.parent_hash;
                let block_number = block.header.number.unwrap();
                let safe_block_hash = block_provider
                    .get_block_by_number((block_number - 32).into(), false)
                    .await
                    .unwrap()
                    .expect("safe block exists")
                    .header
                    .hash
                    .expect("safe block has hash");

                let finalized_block_hash = block_provider
                    .get_block_by_number((block_number - 64).into(), false)
                    .await
                    .unwrap()
                    .expect("finalized block exists")
                    .header
                    .hash
                    .expect("finalized block has hash");

                next_block += 1;
                sender
                    .send((
                        head_block_hash,
                        safe_block_hash,
                        finalized_block_hash,
                        block.transactions,
                        payload_attributes,
                    ))
                    .await
                    .unwrap();
            }
        });

        // put results in a summary vec so they can be printed at the end
        // TODO: just accumulate on the fly
        let mut results = Vec::new();

        while let Some((head, safe, finalized, transactions, payload_attributes)) =
            receiver.recv().await
        {
            // debug!(
            //     number=?payload.payload_inner.payload_inner.block_number,
            //     hash=?payload.payload_inner.payload_inner.block_hash,
            //     parent_hash=?payload.payload_inner.payload_inner.parent_hash,
            //     "Sending payload",
            // );

            // construct fcu to call
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let start = Instant::now();
            let fcu_result = auth_provider
                .fork_choice_updated_v3_wait(forkchoice_state, Some(payload_attributes))
                .await?;
            let payload_id = fcu_result.payload_id.unwrap();
            let fcu_duration = start.elapsed();

            for tx in transactions.as_transactions().unwrap() {
                let mut buffer = Vec::new();

                // convert tx to envelope
                let tx = TxEnvelope::try_from(tx.clone()).unwrap();
                tx.encode_2718(&mut buffer);

                // we don't actually care about the returned future
                let _ = auth_provider.send_raw_transaction(&buffer).await?;
            }

            // sleep?
            tokio::time::sleep(Duration::from_secs(1)).await;

            let start = Instant::now();

            // send getPayload
            let res = auth_provider.get_payload_v3(payload_id).await?;
            let get_payload_duration = start.elapsed();

            // let versioned_hashes: Vec<B256> =
            //     block.blob_versioned_hashes().into_iter().copied().collect();
            // let (payload, parent_beacon_block_root) = block_to_payload_v3(block);

            // now send newPayload
            // auth_provider.new_payload_v3_wait(res, vec![],
            // res.payload_inner.parent_beacon_block_root).await?;

            // convert gas used to gigagas, then compute gigagas per second
            // let gigagas_used = gas_used / 1_000_000_000.0;
            // let gigagas_per_second = gigagas_used / new_payload_duration.as_secs_f64();
            // results.push((new_payload_duration, gas_used));
            // info!(
            //     ?fcu_duration,
            //     ?new_payload_duration,
            //     "New payload processed at {:.2} Ggas/s, used {} total gas",
            //     gigagas_per_second,
            //     gas_used
            // );
        }

        // accumulate the results and calculate the overall Ggas/s
        let total_gas_used: f64 = results.iter().map(|(_, gas)| gas).sum();
        let total_duration: Duration = results.iter().map(|(dur, _)| dur).sum();
        let total_ggas_used = total_gas_used / 1_000_000_000.0;
        let total_ggas_per_second = total_ggas_used / total_duration.as_secs_f64();
        info!(?total_duration, ?total_gas_used, "Total Ggas/s: {:.2}", total_ggas_per_second);

        Ok(())
    }
}
