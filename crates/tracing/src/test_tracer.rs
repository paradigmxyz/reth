use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::Tracer;

///  Initializes a tracing subscriber for tests.
///
///  The filter is configurable via `RUST_LOG`.
///
///  # Note
///
///  The subscriber will silently fail if it could not be installed.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TestTracer;

impl Tracer for TestTracer {
    fn init(self) -> eyre::Result<Option<WorkerGuard>> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
        Ok(None)
    }
}
