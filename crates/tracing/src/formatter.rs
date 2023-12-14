use clap::ValueEnum;
use std::{fmt, fmt::Display};
use tracing_appender::non_blocking::NonBlocking;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::BoxedLayer;

/// The log format
#[derive(Debug, Copy, Clone, ValueEnum, Eq, PartialEq)]
pub enum LogFormat {
    /// Json formatting
    Json,
    /// Logfmt formatting
    LogFmt,
    /// Terminal formatting
    Terminal,
}

impl LogFormat {
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
        let target = std::env::var("RUST_LOG_TARGET").map(|val| val != "0").unwrap_or(true);

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
