//! This exposes the process CLI arguments over prometheus.
use metrics::gauge;

/// Registers the process CLI arguments as a prometheus metric.
///
/// This captures all arguments passed to the binary via [`std::env::args`] and emits them as a
/// `process_cli_args` gauge set to `1` with an `args` label containing the full command line.
pub fn register_process_metrics() {
    let args: String = std::env::args().collect::<Vec<_>>().join(" ");
    let gauge = gauge!("process_cli_args", "args" => args);
    gauge.set(1);
}
