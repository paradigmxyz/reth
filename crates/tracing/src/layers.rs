use std::path::Path;

use rolling_file::{RollingConditionBasic, RollingFileAppender};
use tracing::Subscriber;
use tracing_subscriber::{registry::LookupSpan, EnvFilter, Layer, Registry};

use crate::{formatter::LogFormat, BoxedLayer, FileWorkerGuard};

pub struct Layers {
    inner: Vec<BoxedLayer<Registry>>,
}

/// Default [directives](Directive) for [EnvFilter] which disables high-frequency debug logs from
/// `hyper` and `trust-dns`
const DEFAULT_ENV_FILTER_DIRECTIVES: [&str; 3] =
    ["hyper::proto::h1=off", "trust_dns_proto=off", "atrust_dns_resolver=off"];

fn env_filter(directives: &str) -> eyre::Result<EnvFilter> {
    let env_filter = EnvFilter::builder().from_env_lossy();

    DEFAULT_ENV_FILTER_DIRECTIVES
        .into_iter()
        .chain(directives.split(','))
        .try_fold(env_filter, |env_filter, directive| {
            Ok(env_filter.add_directive(directive.parse()?))
        })
}

impl Layers {
    pub fn new() -> Self {
        Self { inner: vec![] }
    }
    pub fn into_inner(self) -> Vec<BoxedLayer<Registry>> {
        self.inner
    }

    pub fn journald(&mut self, filter: &str) {
        let journald_filter = env_filter(filter).unwrap();
        let layer = tracing_journald::layer().unwrap().with_filter(journald_filter).boxed();
        self.inner.push(layer);
    }

    pub fn stdout<S>(&mut self, format: LogFormat, filter: &str, color: &str)
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let filter = env_filter(filter).unwrap();
        let layer = format.apply(filter, None);
        self.inner.push(layer.boxed());
    }

    pub fn file(
        &mut self,
        format: LogFormat,
        filter: &str,
        dir: impl AsRef<Path>,
        file_name: impl AsRef<Path>,
        max_size_bytes: u64,
        max_files: usize,
    ) -> FileWorkerGuard {
        let file_filter = env_filter(filter).unwrap();
        // Create log dir if it doesn't exist (RFA doesn't do this for us)
        let log_dir = dir.as_ref();
        if !log_dir.exists() {
            std::fs::create_dir_all(log_dir).expect("Could not create log directory");
        }

        // Create layer
        let (writer, guard) = tracing_appender::non_blocking(
            RollingFileAppender::new(
                log_dir.join(file_name.as_ref()),
                RollingConditionBasic::new().max_size(max_size_bytes),
                max_files,
            )
            .expect("Could not initialize file logging"),
        );
        let layer = format.apply(file_filter, Some(writer));
        self.inner.push(layer);
        guard
    }
}
