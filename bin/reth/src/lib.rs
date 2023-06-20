#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Rust Ethereum (reth) binary executable.
//!
//! ## Feature Flags
//!
//! - `jemalloc`: Uses [jemallocator](https://github.com/tikv/jemallocator) as the global allocator.
//!   This is **not recommended on Windows**. See [here](https://rust-lang.github.io/rfcs/1974-global-allocators.html#jemalloc)
//!   for more info.
//! - `only-info-logs`: Disables all logs below `info` level. This can speed up the node, since
//!   fewer calls to the logging component is made.

pub mod args;
pub mod chain;
pub mod cli;
pub mod config;
pub mod db;
pub mod debug_cmd;
pub mod dirs;
pub mod node;
pub mod p2p;
pub mod prometheus_exporter;
pub mod runner;
pub mod stage;
pub mod test_vectors;
pub mod utils;
pub mod version;

#[cfg(feature = "jemalloc")]
use jemallocator as _;
