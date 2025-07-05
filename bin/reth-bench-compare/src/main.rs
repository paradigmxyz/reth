//! # reth-bench-compare
//!
//! Automated tool for comparing reth performance between two git branches.
//! This tool automates the complete workflow of compiling, running, and benchmarking
//! reth on different branches to provide meaningful performance comparisons.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

mod benchmark;
mod cli;
mod comparison;
mod compilation;
mod git;
mod node;

use clap::Parser;
use cli::{run_comparison, Args};
use eyre::Result;
use reth_cli_runner::CliRunner;

fn main() -> Result<()> {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    let args = Args::parse();

    // Initialize tracing
    let _guard = args.init_tracing()?;

    // Run until either exit or sigint or sigterm
    let runner = CliRunner::try_default_runtime()?;
    runner.run_command_until_exit(|ctx| run_comparison(args, ctx))
}
