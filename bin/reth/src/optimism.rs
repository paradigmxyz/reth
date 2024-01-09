#![allow(missing_docs, rustdoc::missing_crate_level_docs)]
// We use jemalloc for performance reasons
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[cfg(not(feature = "optimism"))]
compile_error!("Cannot build the `op-reth` binary with the `optimism` feature flag disabled. Did you mean to build `reth`?");

#[cfg(feature = "optimism")]
fn main() {
    // Enable backtraces unless a RUST_BACKTRACE value has already been explicitly provided.
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
