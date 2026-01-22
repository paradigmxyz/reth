use tracing_subscriber::EnvFilter;

use crate::{Layers, Tracer, TracingInitResult};

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
    fn init_with_layers_and_reload(
        self,
        _layers: Layers,
        _enable_reload: bool,
    ) -> eyre::Result<TracingInitResult> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
        Ok(TracingInitResult { file_guard: None })
    }
}
