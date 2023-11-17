// We use jemalloc for performance reasons
#[cfg(all(feature = "jemalloc", unix))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

#[cfg(not(feature = "optimism"))]
compile_error!("Cannot build the `op-reth` binary with the `optimism` feature flag disabled. Did you mean to build `reth`?");

#[cfg(feature = "optimism")]
fn main() {
    print_canyon_warning();
    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}

#[inline]
fn print_canyon_warning() {
    println!("---------------------- [ WARNING! ] ----------------------");
    println!("`op-reth` does not currently support the Canyon Hardfork,");
    println!("which went live on 2023-14-11 12PM EST on Sepolia and Goerli.");
    println!("The node will cease to sync at that blocktime (1699981200).");
    println!("Please consult the Canyon Hardfork tracking issue to follow");
    println!("along with the progress of the hardfork implementation:");
    println!("https://github.com/paradigmxyz/reth/issues/5210");
    println!("----------------------------------------------------------\n");
}
