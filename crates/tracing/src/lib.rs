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
//!          LevelFilter::INFO.to_string(),
//!          "debug".to_string(),
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
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

// Re-export tracing crates
pub use tracing;
pub use tracing_appender;
pub use tracing_subscriber;

// Re-export our types
pub use formatter::LogFormat;
pub use layers::{FileInfo, FileWorkerGuard, ReloadableOtlpHandle};
pub use test_tracer::TestTracer;

mod formatter;
mod layers;
#[cfg(feature = "opentelemetry")]
mod otlp;
mod test_tracer;

#[cfg(feature = "opentelemetry")]
pub use otlp::{OtlpConfig, OtlpProtocols};

use crate::layers::Layers;
use core::fmt;
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
    #[cfg(feature = "opentelemetry")]
    otlp: Option<OtlpConfig>,
}

impl RethTracer {
    ///  Constructs a new `Tracer` with default settings.
    ///
    ///  Initializes with default stdout layer configuration.
    ///  Journald and file layers are not set by default.
    pub fn new() -> Self {
        Self {
            stdout: LayerInfo::default(),
            journald: None,
            file: None,
            #[cfg(feature = "opentelemetry")]
            otlp: None,
        }
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

    /// Sets an OTLP configuration.
    #[cfg(feature = "opentelemetry")]
    pub fn with_otlp(mut self, config: OtlpConfig) -> Self {
        self.otlp = Some(config);
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
    default_directive: String,
    filters: String,
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
    ///  * `default_directive` - Directive for filtering log messages.
    ///  * `filters` - Additional filtering parameters as a string.
    ///  * `color` - Optional color configuration for the log messages.
    pub const fn new(
        format: LogFormat,
        default_directive: String,
        filters: String,
        color: Option<String>,
    ) -> Self {
        Self { format, default_directive, filters, color }
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
            default_directive: LevelFilter::INFO.to_string(),
            filters: String::new(),
            color: Some("always".to_string()),
        }
    }
}

/// Tracer handle for managing the logging configuration.
pub struct TracerHandle {
    /// Guard for the file layer, if any
    pub file_guard: Option<WorkerGuard>,
    /// Handle for the tracing subscriber, to allow reloading after
    /// tokio runtime initialization.
    #[cfg(feature = "opentelemetry")]
    otlp_handle: Option<ReloadableOtlpHandle>,
    /// Handle for the tracing subscriber, to allow reloading after
    #[cfg(feature = "opentelemetry")]
    otlp_config: Option<OtlpConfig>,
}

impl Default for TracerHandle {
    fn default() -> Self {
        Self {
            file_guard: None,
            #[cfg(feature = "opentelemetry")]
            otlp_handle: None,
            #[cfg(feature = "opentelemetry")]
            otlp_config: None,
        }
    }
}

impl fmt::Debug for TracerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracerHandle")
            .field("file_guard", &self.file_guard)
            .field("otlp_config", &self.otlp_config)
            .finish_non_exhaustive()
    }
}

impl TracerHandle {
    /// Reloads the otlp layer if configured. If not configured, this does
    /// nothing. This allows OTLP to be lazily initalized after the tokio
    /// runtime has started.
    #[cfg(feature = "opentelemetry")]
    pub fn reload_otlp(&self) -> eyre::Result<()> {
        if let Some(handle) = &self.otlp_handle {
            use opentelemetry::trace::TracerProvider;
            use opentelemetry_otlp::{ExportConfig, WithExportConfig};
            use tracing_subscriber::Layer;

            let otlp = self.otlp_config.as_ref().expect("otlp config not set");

            let filter = layers::build_env_filter(None, &otlp.directive)?;
            let exporter = opentelemetry_otlp::new_exporter()
                .http()
                .with_export_config(ExportConfig {
                    endpoint: otlp.url.clone(),
                    protocol: otlp.protocol.into(),
                    timeout: std::time::Duration::from_millis(otlp.timeout),
                })
                .with_http_client(reqwest::Client::default());

            let tracer = opentelemetry_otlp::new_pipeline()
                .tracing()
                .with_exporter(exporter)
                .install_batch(opentelemetry_sdk::runtime::Tokio)?
                .tracer("reth");

            let layer =
                tracing_opentelemetry::layer().with_tracer(tracer).with_filter(filter).boxed();

            handle.modify(|existing| {
                *existing = layer;
            })?;
        }
        Ok(())
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
    fn init(self) -> eyre::Result<TracerHandle>;
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
    fn init(self) -> eyre::Result<TracerHandle> {
        let mut layers = Layers::new();

        layers.stdout(
            self.stdout.format,
            self.stdout.default_directive.parse()?,
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

        #[cfg(feature = "opentelemetry")]
        let otlp_handle: Option<ReloadableOtlpHandle> =
            self.otlp.as_ref().map(|_| layers.otlp_placeholder());

        // The error is returned if the global default subscriber is already set,
        // so it's safe to ignore it
        let _ =
            tracing_subscriber::registry().with(layers.into_inner()).try_init().inspect_err(|e| {
                tracing::warn!(
                    %e,
                    "Tracing subscriber could not be initialized. This may be a reth bug."
                )
            });
        Ok(TracerHandle {
            file_guard,
            #[cfg(feature = "opentelemetry")]
            otlp_handle,
            #[cfg(feature = "opentelemetry")]
            otlp_config: self.otlp,
        })
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
