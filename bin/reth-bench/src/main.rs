//! # reth-benchmark
//!
//! This is a tool that converts existing blocks into a stream of blocks for benchmarking purposes.
//! These blocks are then fed into reth as a stream of execution payloads.

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

use clap::Parser;
use reth_bench::bench::BenchmarkCommand;
use reth_cli_runner::CliRunner;

fn main() -> eyre::Result<()> {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        unsafe {
            std::env::set_var("RUST_BACKTRACE", "1");
        }
    }

    color_eyre::install()?;

    // Run until either exit or sigint or sigterm
    let runner = CliRunner::try_default_runtime()?;
    runner.run_command_until_exit(|ctx| BenchmarkCommand::parse().execute(ctx))?;

    Ok(())
}
