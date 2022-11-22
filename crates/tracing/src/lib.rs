#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! reth-tracing

// re-export tracing crates.
pub use tracing;
pub use tracing_subscriber;

/// Initialises a tracing subscriber via `RUST_LOG` environment variable filter.
///
/// Note: This ignores any error and should be used for testing.
pub fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
}
