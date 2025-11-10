//! Metrics utilities for the node.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod chain;
/// The metrics hooks for prometheus.
pub mod hooks;
pub mod recorder;
/// The metric server serving the metrics.
pub mod server;
pub mod version;
/// Transaction tracing for monitoring transaction lifecycle (X Layer)
pub mod transaction_trace_xlayer;

pub use metrics_exporter_prometheus::*;
pub use metrics_process::*;
pub use transaction_trace_xlayer::TransactionTracer;
// Re-export transaction trace module items for convenience
pub use transaction_trace_xlayer::{get_global_tracer, init_global_tracer, flush_global_tracer, TransactionProcessId};
