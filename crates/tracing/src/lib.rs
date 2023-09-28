//! Reth tracing subscribers and utilities.
//!
//! Contains a standardized set of layers:
//!
//! - [`stdout()`]
//! - [`file()`]
//! - [`journald()`]
//!
//! As well as a simple way to initialize a subscriber: [`init`].

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(missing_debug_implementations, missing_docs, unreachable_pub, rustdoc::all)]
#![deny(unused_must_use, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use rolling_file::{RollingConditionBasic, RollingFileAppender};
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
/// The events are filtered by `default_directive`, unless overridden by `RUST_LOG`.
///
/// Colors can be disabled with `RUST_LOG_STYLE=never`, and event targets can be displayed with
/// `RUST_LOG_TARGET=1`.
pub fn stdout<S>(default_directive: impl Into<Directive>, color: &str) -> BoxedLayer<S>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    // TODO: Auto-detect
    let with_ansi =
        std::env::var("RUST_LOG_STYLE").map(|val| val != "never").unwrap_or(color != "never");
    let with_target = std::env::var("RUST_LOG_TARGET").map(|val| val != "0").unwrap_or(true);

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
/// The events are filtered by `filter`.
///
/// The boxed layer and a guard is returned. When the guard is dropped the buffer for the log
/// file is immediately flushed to disk. Any events after the guard is dropped may be missed.
#[must_use = "tracing guard must be kept alive to flush events to disk"]
pub fn file<S>(
    filter: EnvFilter,
    dir: impl AsRef<Path>,
    file_name: impl AsRef<Path>,
    max_size_bytes: u64,
    max_files: usize,
) -> (BoxedLayer<S>, tracing_appender::non_blocking::WorkerGuard)
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    // Create log dir if it doesn't exist (RFA doesn't do this for us)
    let log_dir = dir.as_ref();
    if !log_dir.exists() {
        std::fs::create_dir_all(log_dir).expect("Could not create log directory");
    }

    // Create layer
    let (writer, guard) = tracing_appender::non_blocking(
        RollingFileAppender::new(
            log_dir.join(file_name.as_ref()),
            RollingConditionBasic::new().max_size(max_size_bytes),
            max_files,
        )
        .expect("Could not initialize file logging"),
    );
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(writer)
        .with_filter(filter)
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
/// The events are filtered by `filter`.
///
/// If the layer cannot connect to journald for any reason this function will return an error.
pub fn journald<S>(filter: EnvFilter) -> std::io::Result<BoxedLayer<S>>
where
    S: Subscriber,
    for<'a> S: LookupSpan<'a>,
{
    Ok(tracing_journald::layer()?.with_filter(filter).boxed())
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
