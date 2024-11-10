//! clap [Args](clap::Args) for logging configuration.

use crate::dirs::{LogsDir, PlatformPath};
use clap::{ArgAction, Args, ValueEnum};
use reth_tracing::{
    tracing_subscriber::filter::Directive, FileInfo, LayerInfo, LogFormat, RethTracer, Tracer,
    TracerHandle,
};
use std::fmt::{self, Display};
use tracing::{level_filters::LevelFilter, Level};
/// Constant to convert megabytes to bytes
const MB_TO_BYTES: u64 = 1024 * 1024;

/// The log configuration.
#[derive(Debug, Args)]
#[command(next_help_heading = "Logging")]
pub struct LogArgs {
    /// The format to use for logs written to stdout.
    #[arg(long = "log.stdout.format", value_name = "FORMAT", global = true, default_value_t = LogFormat::Terminal)]
    pub log_stdout_format: LogFormat,

    /// The filter to use for logs written to stdout.
    #[arg(long = "log.stdout.filter", value_name = "FILTER", global = true, default_value = "")]
    pub log_stdout_filter: String,

    /// The format to use for logs written to the log file.
    #[arg(long = "log.file.format", value_name = "FORMAT", global = true, default_value_t = LogFormat::Terminal)]
    pub log_file_format: LogFormat,

    /// The filter to use for logs written to the log file.
    #[arg(long = "log.file.filter", value_name = "FILTER", global = true, default_value = "debug")]
    pub log_file_filter: String,

    /// The path to put log files in.
    #[arg(long = "log.file.directory", value_name = "PATH", global = true, default_value_t)]
    pub log_file_directory: PlatformPath<LogsDir>,

    /// The maximum size (in MB) of one log file.
    #[arg(long = "log.file.max-size", value_name = "SIZE", global = true, default_value_t = 200)]
    pub log_file_max_size: u64,

    /// The maximum amount of log files that will be stored. If set to 0, background file logging
    /// is disabled.
    #[arg(long = "log.file.max-files", value_name = "COUNT", global = true, default_value_t = 5)]
    pub log_file_max_files: usize,

    /// Write logs to journald.
    #[arg(long = "log.journald", global = true)]
    pub journald: bool,

    /// The filter to use for logs written to journald.
    #[arg(
        long = "log.journald.filter",
        value_name = "FILTER",
        global = true,
        default_value = "error"
    )]
    pub journald_filter: String,

    /// Sets whether or not the formatter emits ANSI terminal escape codes for colors and other
    /// text formatting.
    #[arg(
        long,
        value_name = "COLOR",
        global = true,
        default_value_t = ColorMode::Always
    )]
    pub color: ColorMode,
    /// The verbosity settings for the tracer.
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[cfg(feature = "opentelemetry")]
    /// Arguments for the `OpenTelemetry` tracing layer.
    #[command(flatten)]
    pub otel: OtelArgs,
}

impl LogArgs {
    /// Creates a [`LayerInfo`] instance.
    fn layer(&self, format: LogFormat, filter: String, use_color: bool) -> LayerInfo {
        LayerInfo::new(
            format,
            self.verbosity.directive().to_string(),
            filter,
            use_color.then(|| self.color.to_string()),
        )
    }

    /// File info from the current log options.
    fn file_info(&self) -> FileInfo {
        FileInfo::new(
            self.log_file_directory.clone().into(),
            self.log_file_max_size * MB_TO_BYTES,
            self.log_file_max_files,
        )
    }

    /// Creates a [`RethTracer`] instance from the current log options.
    pub fn tracer(&self) -> RethTracer {
        let mut tracer = RethTracer::new();

        let stdout = self.layer(self.log_stdout_format, self.log_stdout_filter.clone(), true);
        tracer = tracer.with_stdout(stdout);

        if self.journald {
            tracer = tracer.with_journald(self.journald_filter.clone());
        }

        if self.log_file_max_files > 0 {
            let info = self.file_info();
            let file = self.layer(self.log_file_format, self.log_file_filter.clone(), false);
            tracer = tracer.with_file(file, info);
        }

        #[cfg(feature = "opentelemetry")]
        if self.otel.url.is_some() {
            match self.otel.try_to_otlp_config() {
                Ok(otel) => {
                    tracer = tracer.with_otlp(otel);
                }
                Err(err) => {
                    tracing::warn!(err, "Failed to parse otel config, skipping otel layer");
                }
            }
        }

        tracer
    }

    /// Initializes tracing with the configured options from cli args.
    ///
    /// If file logging is enabled, the returned handle has a guard that must be
    /// kept alive to ensure that all logs are flushed to disk. If OTLP is
    /// enabled, the handle can be used to start the OpenTelemetry layer after
    /// the tokio runtime is initialized.
    pub fn init_tracing(&self) -> eyre::Result<TracerHandle> {
        self.tracer().init()
    }
}

/// Arguments for the `OpenTelemetry` tracing layer.
#[cfg(feature = "opentelemetry")]
#[derive(Debug, Args)]
pub struct OtelArgs {
    /// Endpoint url to which to send spans and events. The endpoint URL must
    /// be a valid URL, including the protocol prefix (http or https) and any
    /// http basic auth information.
    ///
    /// If the endpoint is not specified, events and spans will not be exported
    /// to any external service.
    ///
    /// Available only when compiled with the `opentelemetry` feature.
    #[arg(long = "otel.endpoint", value_name = "ENDPOINT", global = true)]
    pub url: Option<String>,

    /// The protocol to use for sending spans and events. Values are "grpc",
    /// "binary", or "json".
    ///
    /// Available only when compiled with the `opentelemetry` feature.
    #[arg(long = "otel.protocol", value_name = "PROTOCOL", global = true, default_value = "json")]
    pub protocol: reth_tracing::OtlpProtocols,

    /// The filter to use for spans and events exported to the `OpenTelemetry`
    /// layer. Values are expected to be in the [`EnvFilter`]'s directive format
    /// "info".
    ///
    /// To specify `off`, omit the `--otel.url` argument.
    ///
    /// Available only when compiled with the `opentelemetry` feature.
    ///
    /// [`EnvFilter`]: https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives
    #[arg(long = "otel.filter", value_name = "LEVEL", global = true, default_value = "info")]
    pub directive: String,

    /// The timeout for sending spans and events, in milliseconds.
    ///
    /// Available only when compiled with the `opentelemetry` feature.
    #[arg(long = "otel.timeout", value_name = "TIMEOUT", global = true, default_value = "1000")]
    pub timeout: u64,
}

#[cfg(feature = "opentelemetry")]
impl OtelArgs {
    /// Create an `OtlpConfig` from these command line arguments options.
    pub fn try_to_otlp_config(&self) -> Result<reth_tracing::OtlpConfig, &'static str> {
        Ok(reth_tracing::OtlpConfig {
            url: self.url.clone().ok_or("no otel url specified via --otel.url")?,
            protocol: self.protocol,
            directive: self.directive.clone(),
            timeout: self.timeout,
        })
    }
}

/// The color mode for the cli.
#[derive(Debug, Copy, Clone, ValueEnum, Eq, PartialEq)]
pub enum ColorMode {
    /// Colors on
    Always,
    /// Colors on
    Auto,
    /// Colors off
    Never,
}

impl Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Always => write!(f, "always"),
            Self::Auto => write!(f, "auto"),
            Self::Never => write!(f, "never"),
        }
    }
}

/// The verbosity settings for the cli.
#[derive(Debug, Copy, Clone, Args)]
#[command(next_help_heading = "Display")]
pub struct Verbosity {
    /// Set the minimum log level.
    ///
    /// -v      Errors
    /// -vv     Warnings
    /// -vvv    Info
    /// -vvvv   Debug
    /// -vvvvv  Traces (warning: very verbose!)
    #[arg(short, long, action = ArgAction::Count, global = true, default_value_t = 3, verbatim_doc_comment, help_heading = "Display")]
    verbosity: u8,

    /// Silence all log output.
    #[arg(long, alias = "silent", short = 'q', global = true, help_heading = "Display")]
    quiet: bool,
}

impl Verbosity {
    /// Get the corresponding [Directive] for the given verbosity, or none if the verbosity
    /// corresponds to silent.
    pub fn directive(&self) -> Directive {
        if self.quiet {
            LevelFilter::OFF.into()
        } else {
            let level = match self.verbosity - 1 {
                0 => Level::ERROR,
                1 => Level::WARN,
                2 => Level::INFO,
                3 => Level::DEBUG,
                _ => Level::TRACE,
            };

            level.into()
        }
    }
}
