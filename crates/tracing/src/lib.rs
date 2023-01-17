#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! reth tracing subscribers and utilities.
//!
//! Contains a standardized set of layers:
//!
//! - [`stdout()`]
//! - [`file()`]
//! - [`journald()`]
//!
//! As well as a simple way to initialize a subscriber: [`init`].
use std::path::Path;
use tracing::Subscriber;
use tracing_subscriber::{
    filter::Directive, prelude::*, registry::LookupSpan, EnvFilter, Layer, Registry,
};

// Re-export tracing crates
pub use tracing;
pub use tracing_subscriber;

/// A boxed tracing [Layer].
pub type BoxedLayer<S> = Box<dyn Layer<S> + Send + Sync>;

/// Initializes a new [Subscriber] based on the given layers.
pub fn init(layers: Vec<BoxedLayer<Registry>>) {
    tracing_subscriber::registry().with(layers).init();
}

/// Builds a new tracing layer that writes to stdout.
///
/// The events are filtered by `default_directive`, unless overriden by `RUST_LOG`.
///
/// Colors can be disabled with `RUST_LOG_STYLE=never`, and event targets can be displayed with
/// `RUST_LOG_TARGET=1`.
pub fn stdout<S>(default_directive: impl Into<Directive>) -> BoxedLayer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    // TODO: Auto-detect
    let with_ansi = std::env::var("RUST_LOG_STYLE").map(|val| val != "never").unwrap_or(true);
    let with_target = std::env::var("RUST_LOG_TARGET").map(|val| val != "0").unwrap_or(false);

    let filter =
        EnvFilter::builder().with_default_directive(default_directive.into()).from_env_lossy();

    tracing_subscriber::fmt::layer()
        .with_ansi(with_ansi)
        .with_target(with_target)
        .with_filter(filter)
        .boxed()
}

/// Builds a new tracing layer that appends to a log file.
///
/// The events are filtered by `directive`.
///
/// The boxed layer and a guard is returned. When the guard is dropped the buffer for the log
/// file is immediately flushed to disk. Any events after the guard is dropped may be missed.
pub fn file<S>(
    directive: impl Into<Directive>,
    dir: impl AsRef<Path>,
    file_name: impl AsRef<Path>,
) -> (BoxedLayer<S>, tracing_appender::non_blocking::WorkerGuard)
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(dir, file_name));
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(EnvFilter::default().add_directive(directive.into()))
        .boxed();

    (layer, guard)
}

/// A worker guard returned by [`file()`].
///
/// When a guard is dropped, all events currently in-memory are flushed to the log file this guard
/// belongs to.
pub type FileWorkerGuard = tracing_appender::non_blocking::WorkerGuard;

/// Builds a new tracing layer that writes events to journald.
///
/// The events are filtered by `directive`.
///
/// If the layer cannot connect to journald for any reason this function will return an error.
pub fn journald<S>(directive: impl Into<Directive>) -> std::io::Result<BoxedLayer<S>>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    Ok(tracing_journald::layer()?
        .with_filter(EnvFilter::default().add_directive(directive.into()))
        .boxed())
}

/// Initializes a tracing subscriber for tests.
///
/// The filter is configurable via `RUST_LOG`.
///
/// # Note
///
/// The subscriber will silently fail if it could not be installed.
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
}
