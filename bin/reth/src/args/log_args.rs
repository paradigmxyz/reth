//! clap [Args](clap::Args) for logging configuration.

use crate::dirs::{LogsDir, PlatformPath};
use clap::{Args, ValueEnum};
use reth_tracing::{
    tracing_subscriber::{registry::LookupSpan, EnvFilter},
    BoxedLayer, FileWorkerGuard,
};
use std::{fmt, fmt::Display};
use tracing::Subscriber;

/// Default [directives](reth_tracing::tracing_subscriber::filter::Directive) for [EnvFilter] which
/// disables high-frequency debug logs from `hyper` and `trust-dns`
const DEFAULT_ENV_FILTER_DIRECTIVES: [&str; 3] =
    ["hyper::proto::h1=off", "trust_dns_proto=off", "atrust_dns_resolver=off"];

/// Constant to convert megabytes to bytes
const MB_TO_BYTES: u64 = 1024 * 1024;

/// The log configuration.
#[derive(Debug, Args)]
#[command(next_help_heading = "Logging")]
pub struct LogArgs {
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

    /// The filter to use for logs written to the log file.
    #[arg(long = "log.file.filter", value_name = "FILTER", global = true, default_value = "debug")]
    pub log_file_filter: String,

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
}

impl LogArgs {
    /// Builds tracing layers from the current log options.
    pub fn layers<S>(&self) -> eyre::Result<(Vec<BoxedLayer<S>>, Option<FileWorkerGuard>)>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let mut layers = Vec::new();

        // Function to create a new EnvFilter with environment (from `RUST_LOG` env var), default
        // (from `DEFAULT_DIRECTIVES`) and additional directives.
        let create_env_filter = |additional_directives: &str| -> eyre::Result<EnvFilter> {
            let env_filter = EnvFilter::builder().from_env_lossy();

            DEFAULT_ENV_FILTER_DIRECTIVES
                .into_iter()
                .chain(additional_directives.split(','))
                .try_fold(env_filter, |env_filter, directive| {
                    Ok(env_filter.add_directive(directive.parse()?))
                })
        };

        // Create and add the journald layer if enabled
        if self.journald {
            let journald_filter = create_env_filter(&self.journald_filter)?;
            layers.push(
                reth_tracing::journald(journald_filter).expect("Could not connect to journald"),
            );
        }

        // Create and add the file logging layer if enabled
        let file_guard = if self.log_file_max_files > 0 {
            let file_filter = create_env_filter(&self.log_file_filter)?;
            let (layer, guard) = reth_tracing::file(
                file_filter,
                &self.log_file_directory,
                "reth.log",
                self.log_file_max_size * MB_TO_BYTES,
                self.log_file_max_files,
            );
            layers.push(layer);
            Some(guard)
        } else {
            None
        };

        Ok((layers, file_guard))
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
            ColorMode::Always => write!(f, "always"),
            ColorMode::Auto => write!(f, "auto"),
            ColorMode::Never => write!(f, "never"),
        }
    }
}
