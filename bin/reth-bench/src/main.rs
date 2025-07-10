//! # reth-benchmark
//!
//! This is a tool that converts existing blocks into a stream of blocks for benchmarking purposes.
//! These blocks are then fed into reth as a stream of execution payloads.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

pub mod authenticated_transport;
pub mod bench;
pub mod bench_mode;
pub mod valid_payload;

use alloy_provider::network::AnyRpcBlock;
use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadInputV2};
use bench::BenchmarkCommand;
use clap::Parser;
use eyre::Ok;
use reth_cli_runner::{CliContext, CliRunner};

use crate::bench::EngineRequests;

fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Run until either exit or sigint or sigterm
    let runner = CliRunner::try_default_runtime().unwrap();

    let convert = |block: AnyRpcBlock| -> eyre::Result<EngineRequests> {
        let block = block
            .into_inner()
            .map_header(|header| header.map(|h| h.into_header_with_defaults()))
            .try_map_transactions(|tx| tx.try_into_either::<op_alloy_consensus::OpTxEnvelope>())?
            .into_consensus();

        let (payload, sidecar) = ExecutionPayload::from_block_slow(&block);

        let (method, fcu_method, params) = match payload {
            ExecutionPayload::V3(payload) => {
                let cancun = sidecar.cancun().unwrap();

                if let Some(prague) = sidecar.prague() {
                    (
                        "engine_newPayloadV4",
                        "engine_forkchoiceUpdatedV3",
                        serde_json::to_value((
                            payload,
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                            prague.requests.requests_hash(),
                        ))?,
                    )
                } else {
                    (
                        "engine_newPayloadV3",
                        "engine_forkchoiceUpdatedV3",
                        serde_json::to_value((
                            payload,
                            cancun.versioned_hashes.clone(),
                            cancun.parent_beacon_block_root,
                        ))?,
                    )
                }
            }
            ExecutionPayload::V2(payload) => {
                let input = ExecutionPayloadInputV2 {
                    execution_payload: payload.payload_inner,
                    withdrawals: Some(payload.withdrawals),
                };

                (
                    "engine_newPayloadV2",
                    "engine_forkchoiceUpdatedV2",
                    serde_json::to_value((input,))?,
                )
            }
            ExecutionPayload::V1(payload) => (
                "engine_newPayloadV1",
                "engine_forkchoiceUpdatedV1",
                serde_json::to_value((payload,))?,
            ),
        };
        Ok(EngineRequests { method: method.into(), params, fcu_method: fcu_method.into() })
    };

    runner
        .run_command_until_exit(|ctx: CliContext| {
            let command = BenchmarkCommand::parse();
            command.execute(ctx, convert)
        })
        .unwrap();
}
