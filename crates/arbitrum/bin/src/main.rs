#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use tracing::info;

#[global_allocator]
static ALLOC: reth_cli_util::allocator::Allocator = reth_cli_util::allocator::new_allocator();

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    info!(target: "arb-reth::cli", "arb-reth placeholder binary starting");
    println!("arb-reth: Arbitrum Reth CLI scaffold");
}
