//!  The `tracing` module provides functionalities for setting up and configuring logging.
//!
//!  It includes structures and functions to create and manage various logging layers: stdout,
//!  file, or journald. The module's primary entry point is the `Tracer` struct, which can be
//!  configured to use different logging formats and destinations. If no layer is specified, it will
//!  default to stdout.
//!
//!  # Examples
//!
//!  Basic usage:
//!
//!  ```
//!  use reth_tracing::{
//!      LayerInfo, RethTracer, Tracer,
//!      tracing::level_filters::LevelFilter,
//!      LogFormat,
//!  };
//!
//!  fn main() -> eyre::Result<()> {
//!      let tracer = RethTracer::new().with_stdout(LayerInfo::new(
//!          LogFormat::Json,
//!          "debug".to_string(),
//!          LevelFilter::INFO.into(),
//!          None,
//!      ));
//!
//!      tracer.init()?;
//!
//!      // Your application logic here
//!
//!      Ok(())
//!  }
//!  ```
//!
//!  This example sets up a tracer with JSON format logging for journald and terminal-friendly
//! format  for file logging.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use tracing_subscriber::filter::Directive;

// Re-export tracing crates
pub use tracing;
pub use tracing_subscriber;

// Re-export LogFormat
pub use formatter::LogFormat;
pub use layers::{FileInfo, FileWorkerGuard};

pub use test_tracer::TestTracer;

mod formatter;
mod layers;
mod test_tracer;

use crate::layers::Layers;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

///  Tracer for application logging.
///
///  Manages the configuration and initialization of logging layers,
/// including standard output, optional journald, and optional file logging.
#[derive(Debug, Clone)]
pub struct RethTracer {
    stdout: LayerInfo,
    journald: Option<String>,
    file: Option<(LayerInfo, FileInfo)>,
}

impl RethTracer {
    ///  Constructs a new `Tracer` with default settings.
    ///
    ///  Initializes with default stdout layer configuration.
    ///  Journald and file layers are not set by default.
    pub fn new() -> Self {
        Self { stdout: LayerInfo::default(), journald: None, file: None }
    }

    ///  Sets a custom configuration for the stdout layer.
    ///
    ///  # Arguments
    ///  * `config` - The `LayerInfo` to use for the stdout layer.
    pub fn with_stdout(mut self, config: LayerInfo) -> Self {
        self.stdout = config;
        self
    }

    ///  Sets the journald layer filter.
    ///
    ///  # Arguments
    ///  * `filter` - The `filter` to use for the journald layer.
    pub fn with_journald(mut self, filter: String) -> Self {
        self.journald = Some(filter);
        self
    }

    ///  Sets the file layer configuration and associated file info.
    ///
    ///  # Arguments
    ///  * `config` - The `LayerInfo` to use for the file layer.
    ///  * `file_info` - The `FileInfo` containing details about the log file.
    pub fn with_file(mut self, config: LayerInfo, file_info: FileInfo) -> Self {
        self.file = Some((config, file_info));
        self
    }
}

impl Default for RethTracer {
    fn default() -> Self {
        Self::new()
    }
}

///  Configuration for a logging layer.
///
///  This struct holds configuration parameters for a tracing layer, including
///  the format, filtering directives, optional coloring, and directive.
#[derive(Debug, Clone)]
pub struct LayerInfo {
    format: LogFormat,
    filters: String,
    directive: Directive,
    color: Option<String>,
}

impl LayerInfo {
    ///  Constructs a new `LayerInfo`.
    ///
    ///  # Arguments
    ///  * `format` - Specifies the format for log messages. Possible values are:
    ///      - `LogFormat::Json` for JSON formatting.
    ///      - `LogFormat::LogFmt` for logfmt (key=value) formatting.
    ///      - `LogFormat::Terminal` for human-readable, terminal-friendly formatting.
    ///  * `filters` - Additional filtering parameters as a string.
    ///  * `directive` - Directive for filtering log messages.
    ///  * `color` - Optional color configuration for the log messages.
    pub fn new(
        format: LogFormat,
        filters: String,
        directive: Directive,
        color: Option<String>,
    ) -> Self {
        Self { format, directive, filters, color }
    }
}

impl Default for LayerInfo {
    ///  Provides default values for `LayerInfo`.
    ///
    ///  By default, it uses terminal format, INFO level filter,
    ///  no additional filters, and no color configuration.
    fn default() -> Self {
        Self {
            format: LogFormat::Terminal,
            directive: LevelFilter::INFO.into(),
            filters: "debug".to_string(),
            color: Some("always".to_string()),
        }
    }
}

/// Trait defining a general interface for logging configuration.
///
/// The `Tracer` trait provides a standardized way to initialize logging configurations
/// in an application. Implementations of this trait can specify different logging setups,
/// such as standard output logging, file logging, journald logging, or custom logging
/// configurations tailored for specific environments (like testing).
pub trait Tracer {
    /// Initialize the logging configuration.
    ///  # Returns
    ///  An `eyre::Result` which is `Ok` with an optional `WorkerGuard` if a file layer is used,
    ///  or an `Err` in case of an error during initialization.
    fn init(self) -> eyre::Result<Option<WorkerGuard>>;
}

impl Tracer for RethTracer {
    ///  Initializes the logging system based on the configured layers.
    ///
    ///  This method sets up the global tracing subscriber with the specified
    ///  stdout, journald, and file layers.
    ///
    ///  The default layer is stdout.
    ///
    ///  # Returns
    ///  An `eyre::Result` which is `Ok` with an optional `WorkerGuard` if a file layer is used,
    ///  or an `Err` in case of an error during initialization.
    fn init(self) -> eyre::Result<Option<WorkerGuard>> {
        let mut layers = Layers::new();

        layers.stdout(
            self.stdout.format,
            self.stdout.directive,
            &self.stdout.filters,
            self.stdout.color,
        )?;

        if let Some(config) = self.journald {
            layers.journald(&config)?;
        }

        let file_guard = if let Some((config, file_info)) = self.file {
            Some(layers.file(config.format, &config.filters, file_info)?)
        } else {
            None
        };

        // The error is returned if the global default subscriber is already set,
        // so it's safe to ignore it
        let _ = tracing_subscriber::registry().with(layers.into_inner()).try_init();
        Ok(file_guard)
    }
}

///  Initializes a tracing subscriber for tests.
///
///  The filter is configurable via `RUST_LOG`.
///
///  # Note
///
///  The subscriber will silently fail if it could not be installed.
pub fn init_test_tracing() {
    let _ = TestTracer::default().init();
}
