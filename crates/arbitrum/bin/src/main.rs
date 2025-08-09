#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

fn main() {
    reth_cli_util::sigsegv_handler::install();

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    println!("arb-reth starting (placeholder)");
}
