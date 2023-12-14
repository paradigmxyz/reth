use std::path::{Path, PathBuf};

use rolling_file::{RollingConditionBasic, RollingFileAppender};
use tracing::Subscriber;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{registry::LookupSpan, EnvFilter, Layer, Registry};

use crate::{formatter::LogFormat, BoxedLayer, FileWorkerGuard};

pub struct Layers {
    inner: Vec<BoxedLayer<Registry>>,
}

/// Default [directives](Directive) for [EnvFilter] which disables high-frequency debug logs from
/// `hyper` and `trust-dns`
const DEFAULT_ENV_FILTER_DIRECTIVES: [&str; 3] =
    ["hyper::proto::h1=off", "trust_dns_proto=off", "atrust_dns_resolver=off"];

fn build_env_filter(directives: &str) -> eyre::Result<EnvFilter> {
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

    pub fn journald(&mut self, filter: &str) -> eyre::Result<()> {
        let journald_filter = build_env_filter(filter)?;
        let layer = tracing_journald::layer().unwrap().with_filter(journald_filter).boxed();
        self.inner.push(layer);
        Ok(())
    }

    pub fn stdout<S>(
        &mut self,
        format: LogFormat,
        filter: &str,
        color: Option<String>,
    ) -> eyre::Result<()>
    where
        S: Subscriber,
        for<'a> S: LookupSpan<'a>,
    {
        let filter = build_env_filter(filter)?;
        let layer = format.apply(filter, color, None);
        self.inner.push(layer.boxed());
        Ok(())
    }

    pub fn file(
        &mut self,
        format: LogFormat,
        filter: String,
        file_info: FileInfo,
    ) -> eyre::Result<FileWorkerGuard> {
        let log_dir = file_info.create_log_dir();
        let (writer, guard) = file_info.create_log_writer(&log_dir);

        let file_filter = build_env_filter(&filter.to_string())?;
        let layer = format.apply(file_filter, None, Some(writer));
        self.inner.push(layer);
        Ok(guard)
    }
}

pub struct FileInfo {
    dir: PathBuf,
    file_name: String,
    max_size_bytes: u64,
    max_files: usize,
}

impl FileInfo {
    fn create_log_dir(&self) -> &Path {
        // Create log dir if it doesn't exist (RFA doesn't do this for us)
        let log_dir: &Path = self.dir.as_ref();
        if !log_dir.exists() {
            std::fs::create_dir_all(log_dir).expect("Could not create log directory");
        }
        log_dir
    }

    fn create_log_writer(
        &self,
        log_dir: &Path,
    ) -> (tracing_appender::non_blocking::NonBlocking, WorkerGuard) {
        let (writer, guard) = tracing_appender::non_blocking(
            RollingFileAppender::new(
                log_dir.join(AsRef::<Path>::as_ref(&self.file_name)),
                RollingConditionBasic::new().max_size(self.max_size_bytes),
                self.max_files,
            )
            .expect("Could not initialize file logging"),
        );
        (writer, guard)
    }
}
