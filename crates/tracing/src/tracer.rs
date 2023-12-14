use crate::{
    formatter::LogFormat,
    layers::{FileInfo, Layers},
};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry};

pub struct LayerConfig {
    format: LogFormat,
    filters: String,
    color: Option<String>,
}

impl LayerConfig {
    pub fn new(format: LogFormat, filters: String, color: Option<String>) -> Self {
        Self { format, filters, color }
    }
}

pub struct Tracer {
    stdout: Option<LayerConfig>,
    journald: Option<LayerConfig>,
    file: Option<(LayerConfig, FileInfo)>,
}

impl Tracer {
    pub fn new() -> Self {
        Self { stdout: None, journald: None, file: None }
    }

    pub fn with_stdout(mut self, config: LayerConfig) -> Self {
        self.stdout = Some(config);
        self
    }

    pub fn with_journald(mut self, config: LayerConfig) -> Self {
        self.journald = Some(config);
        self
    }

    pub fn with_file(mut self, config: LayerConfig, file_info: FileInfo) -> Self {
        self.file = Some((config, file_info));
        self
    }

    pub fn init(self) -> eyre::Result<Option<WorkerGuard>> {
        let mut layers = Layers::new();

        if let Some(config) = self.stdout {
            layers.stdout::<Registry>(config.format, &config.filters, config.color)?;
        }

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
