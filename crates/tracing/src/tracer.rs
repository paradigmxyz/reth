use crate::{
    formatter::LogFormat,
    layers::{FileInfo, Layers},
};
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    filter::Directive, layer::SubscriberExt, util::SubscriberInitExt, Registry,
};

/// Configuration for a logging layer.
///
/// This struct holds configuration parameters for a tracing layer, including
/// the format, filtering directives, optional coloring, and directive.
#[derive(Debug)]
pub struct LayerConfig {
    format: LogFormat,
    filters: String,
    color: Option<String>,
    directive: Directive,
}

impl LayerConfig {
    /// Constructs a new `LayerConfig`.
    ///
    /// # Arguments
    /// * `format` - Specifies the format for log messages. Possible values are:
    ///     - `LogFormat::Json` for JSON formatting.
    ///     - `LogFormat::LogFmt` for logfmt (key=value) formatting.
    ///     - `LogFormat::Terminal` for human-readable, terminal-friendly formatting.
    /// * `directive` - Directive for filtering log messages.
    /// * `filters` - Additional filtering parameters as a string.
    /// * `color` - Optional color configuration for the log messages.
    pub fn new(
        format: LogFormat,
        directive: Directive,
        filters: String,
        color: Option<String>,
    ) -> Self {
        Self { format, directive, filters, color }
    }
}
impl Default for LayerConfig {
    /// Provides default values for `LayerConfig`.
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

/// Tracer for application logging.
///
/// Manages the configuration and initialization of logging layers,
/// including standard output, optional journald, and optional file logging.
#[derive(Debug)]
pub struct Tracer {
    stdout: LayerConfig,
    journald: Option<LayerConfig>,
    file: Option<(LayerConfig, FileInfo)>,
}

impl Tracer {
    /// Constructs a new `Tracer` with default settings.
    ///
    /// Initializes with default stdout layer configuration.
    /// Journald and file layers are not set by default.
    pub fn new() -> Self {
        Self { stdout: LayerConfig::default(), journald: None, file: None }
    }

    /// Sets the journald layer configuration.
    ///
    /// # Arguments
    /// * `config` - The `LayerConfig` to use for the journald layer.
    pub fn with_journald(mut self, config: LayerConfig) -> Self {
        self.journald = Some(config);
        self
    }

    /// Sets the file layer configuration and associated file info.
    ///
    /// # Arguments
    /// * `config` - The `LayerConfig` to use for the file layer.
    /// * `file_info` - The `FileInfo` containing details about the log file.

    pub fn with_file(mut self, config: LayerConfig, file_info: FileInfo) -> Self {
        self.file = Some((config, file_info));
        self
    }

    /// Initializes the logging system based on the configured layers.
    ///
    /// This method sets up the global tracing subscriber with the specified
    /// stdout, journald, and file layers.
    ///
    /// # Returns
    /// An `eyre::Result` which is `Ok` with an optional `WorkerGuard` if a file layer is used,
    /// or an `Err` in case of an error during initialization.
    #[must_use]
    pub fn init(self) -> eyre::Result<Option<WorkerGuard>> {
        let mut layers = Layers::new();

        layers.stdout::<Registry>(
            self.stdout.format,
            self.stdout.directive,
            &self.stdout.filters,
            self.stdout.color,
        )?;

        if let Some(config) = self.journald {
            layers.journald(&config.filters)?;
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
