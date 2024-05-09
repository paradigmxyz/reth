use crate::layers::BoxedLayer;
use clap::ValueEnum;
use std::{fmt, fmt::Display};
use tracing_appender::non_blocking::NonBlocking;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// Represents the logging format.
///
/// This enum defines the supported formats for logging output.
/// It is used to configure the format layer of a tracing subscriber.
#[derive(Debug, Copy, Clone, ValueEnum, Eq, PartialEq)]
pub enum LogFormat {
    /// Represents JSON formatting for logs.
    /// This format outputs log records as JSON objects,
    /// making it suitable for structured logging.
    Json,

    /// Represents logfmt (key=value) formatting for logs.
    /// This format is concise and human-readable,
    /// typically used in command-line applications.
    LogFmt,

    /// Represents terminal-friendly formatting for logs.
    Terminal,
}

impl LogFormat {
    /// Applies the specified logging format to create a new layer.
    ///
    /// This method constructs a tracing layer with the selected format,
    /// along with additional configurations for filtering and output.
    ///
    /// # Arguments
    /// * `filter` - An `EnvFilter` used to determine which log records to output.
    /// * `color` - An optional string that enables or disables ANSI color codes in the logs.
    /// * `file_writer` - An optional `NonBlocking` writer for directing logs to a file.
    ///
    /// # Returns
    /// A `BoxedLayer<Registry>` that can be added to a tracing subscriber.
    pub fn apply(
        &self,
        filter: EnvFilter,
        color: Option<String>,
        file_writer: Option<NonBlocking>,
    ) -> BoxedLayer<Registry> {
        let ansi = if let Some(color) = color {
            std::env::var("RUST_LOG_STYLE").map(|val| val != "never").unwrap_or(color != "never")
        } else {
            false
        };
        let target = std::env::var("RUST_LOG_TARGET")
            // `RUST_LOG_TARGET` always overrides default behaviour
            .map(|val| val != "0")
            .unwrap_or_else(|_|
                // If `RUST_LOG_TARGET` is not set, show target in logs only if the max enabled
                // level is higher than INFO (DEBUG, TRACE)
                filter.max_level_hint().map_or(true, |max_level| max_level > tracing::Level::INFO));

        match self {
            LogFormat::Json => {
                let layer =
                    tracing_subscriber::fmt::layer().json().with_ansi(ansi).with_target(target);

                if let Some(writer) = file_writer {
                    layer.with_writer(writer).with_filter(filter).boxed()
                } else {
                    layer.with_filter(filter).boxed()
                }
            }
            LogFormat::LogFmt => tracing_logfmt::layer().with_filter(filter).boxed(),
            LogFormat::Terminal => {
                let layer = tracing_subscriber::fmt::layer().with_ansi(ansi).with_target(target);

                if let Some(writer) = file_writer {
                    layer.with_writer(writer).with_filter(filter).boxed()
                } else {
                    layer.with_filter(filter).boxed()
                }
            }
        }
    }
}

impl Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LogFormat::Json => write!(f, "json"),
            LogFormat::LogFmt => write!(f, "logfmt"),
            LogFormat::Terminal => write!(f, "terminal"),
        }
    }
}
