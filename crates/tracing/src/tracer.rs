use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry};

use crate::{
    formatter::{self, LogFormat},
    layers::{self, Layers},
};

pub struct Tracer {
    format: LogFormat,
    filters: String,
    enable_file_layer: bool,
    enable_journald_layer: bool,
}

impl Tracer {
    pub fn new() -> Self {
        Self {
            format: LogFormat::Terminal,
            filters: "info".to_string(),
            enable_file_layer: true,
            enable_journald_layer: false,
        }
    }

    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_filters(mut self, filters: String) -> Self {
        self.filters = filters;
        self
    }

    pub fn with_file_layer(mut self, enable: bool) -> Self {
        self.enable_file_layer = enable;
        self
    }

    pub fn with_journald_layer(mut self, enable: bool) -> Self {
        self.enable_journald_layer = enable;
        self
    }

    pub fn init(self) {
        let mut layers = Layers::new();
        layers.stdout::<Registry>(self.format, &self.filters, "auto");
        if self.enable_journald_layer {
            layers.journald(&self.filters);
        }
        tracing_subscriber::registry().with(layers.into_inner()).init();
    }
}
