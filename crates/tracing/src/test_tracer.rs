use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use crate::Tracer;

/// A test-specific implementation of the `Tracer`.
///
/// `TestTracer` is tailored for testing environments where the standard logging configuration may
/// not be suitable. It initializes a simple logging setup that outputs to standard error (stderr),
/// and uses environment variables for filtering log messages. This is useful for observing log
/// output during tests without the need for file or external logging systems.
///
/// It does not return a `WorkerGuard` as file logging is typically not used in test environments.
///
/// # Examples
///
/// Basic usage in a test module:
///
/// ```
/// use my_crate::logging::{TestTracer, Tracer};
///
/// let test_tracer = TestTracer;
/// test_tracer.init().expect("Failed to initialize test tracer");
/// ```
#[derive(Debug)]
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
