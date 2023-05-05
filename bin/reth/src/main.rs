// We use jemalloc for performance reasons
#[cfg(all(not(windows), not(target_env = "musl")))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

fn main() {
    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
