use crate::{
    formatter::LogFormat,
    layers::{FileInfo, Layers},
};
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{filter::Directive, layer::SubscriberExt, util::SubscriberInitExt};

/// Tracer for application logging.
///
/// Manages the configuration and initialization of logging layers,
/// including standard output, optional journald, and optional file logging.
#[derive(Debug)]
pub struct Tracer {
    stdout: LayerInfo,
    journald: Option<String>,
    file: Option<(LayerInfo, FileInfo)>,
}

impl Tracer {
    /// Constructs a new `Tracer` with default settings.
    ///
    /// Initializes with default stdout layer configuration.
    /// Journald and file layers are not set by default.
    pub fn new() -> Self {
        Self { stdout: LayerInfo::default(), journald: None, file: None }
    }

    /// Sets a custom configuration for the stdout layer.
    ///
    /// # Arguments
    /// * `config` - The `LayerInfo` to use for the stdout layer.
    pub fn with_stdout(mut self, config: LayerInfo) -> Self {
        self.stdout = config;
        self
    }

    /// Sets the journald layer filter.
    ///
    /// # Arguments
    /// * `filter` - The `filter` to use for the journald layer.
    pub fn with_journald(mut self, filter: String) -> Self {
        self.journald = Some(filter);
        self
    }

    /// Sets the file layer configuration and associated file info.
    ///
    /// # Arguments
    /// * `config` - The `LayerInfo` to use for the file layer.
    /// * `file_info` - The `FileInfo` containing details about the log file.

    pub fn with_file(mut self, config: LayerInfo, file_info: FileInfo) -> Self {
        self.file = Some((config, file_info));
        self
    }

    /// Initializes the logging system based on the configured layers.
    ///
    /// This method sets up the global tracing subscriber with the specified
    /// stdout, journald, and file layers.
    ///
    /// The default layer is stdout.
    ///
    /// # Returns
    /// An `eyre::Result` which is `Ok` with an optional `WorkerGuard` if a file layer is used,
    /// or an `Err` in case of an error during initialization.
    #[must_use = "This method must be called to initialize the logging system."]
    pub fn init(self) -> eyre::Result<Option<WorkerGuard>> {
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
            Some(layers.file(config.format, config.filters, file_info)?)
        } else {
            None
        };

        tracing_subscriber::registry().with(layers.into_inner()).init();
        Ok(file_guard)
    }
}

impl Default for Tracer {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for a logging layer.
///
/// This struct holds configuration parameters for a tracing layer, including
/// the format, filtering directives, optional coloring, and directive.
#[derive(Debug)]
pub struct LayerInfo {
    format: LogFormat,
    filters: String,
    directive: Directive,
    color: Option<String>,
}

impl LayerInfo {
    /// Constructs a new `LayerInfo`.
    ///
    /// # Arguments
    /// * `format` - Specifies the format for log messages. Possible values are:
    ///     - `LogFormat::Json` for JSON formatting.
    ///     - `LogFormat::LogFmt` for logfmt (key=value) formatting.
    ///     - `LogFormat::Terminal` for human-readable, terminal-friendly formatting.
    /// * `filters` - Additional filtering parameters as a string.
    /// * `directive` - Directive for filtering log messages.
    /// * `color` - Optional color configuration for the log messages.
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
    /// Provides default values for `LayerInfo`.
    ///
    /// By default, it uses terminal format, INFO level filter,
    /// no additional filters, and no color configuration.
    fn default() -> Self {
        Self {
            format: LogFormat::Terminal,
            directive: LevelFilter::INFO.into(),
            filters: "".to_string(),
            color: None,
        }
    }
}
