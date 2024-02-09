//! clap [Args](clap::Args) for logging configuration.

use crate::dirs::{LogsDir, PlatformPath};
use clap::{ArgAction, Args, ValueEnum};
use reth_tracing::{
    tracing_subscriber::filter::Directive, FileInfo, FileWorkerGuard, LayerInfo, LogFormat,
    RethTracer, Tracer,
};
use std::{fmt, fmt::Display};
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
    #[clap(flatten)]
    pub verbosity: Verbosity,
}

impl LogArgs {
    /// Creates a [LayerInfo] instance.
    fn layer(&self, format: LogFormat, filter: String, use_color: bool) -> LayerInfo {
        LayerInfo::new(
            format,
            self.verbosity.directive().to_string(),
            filter,
            if use_color { Some(self.color.to_string()) } else { None },
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

    /// Initializes tracing with the configured options from cli args.
    pub fn init_tracing(&self) -> eyre::Result<Option<FileWorkerGuard>> {
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

        let guard = tracer.init()?;
        Ok(guard)
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
    #[clap(short, long, action = ArgAction::Count, global = true, default_value_t = 3, verbatim_doc_comment, help_heading = "Display")]
    verbosity: u8,

    /// Silence all log output.
    #[clap(long, alias = "silent", short = 'q', global = true, help_heading = "Display")]
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
