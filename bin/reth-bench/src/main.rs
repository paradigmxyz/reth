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

use bench::BenchmarkCommand;
use clap::Parser;
use reth_cli_runner::CliRunner;

fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Run until either exit or sigint or sigterm
    let runner = CliRunner::default();
    runner
        .run_command_until_exit(|ctx| {
            let command = BenchmarkCommand::parse();
            command.execute(ctx)
        })
        .unwrap();
}
