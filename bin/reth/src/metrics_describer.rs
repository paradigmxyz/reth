//! Metrics describer
 
use metrics::{describe_counter, describe_histogram};

pub(crate) fn describe_metrics() {
    // Describe stagedsync headers metrics
    describe_counter!("stages.headers.counter", "Number of headers successfully retrived");
    describe_counter!("stages.headers.timeout_errors", "Number of timeout errors while requesting headers");
    describe_counter!("stages.headers.validation_errors", "Number of validation errores while requesting headers");
    describe_counter!("stages.headers.unexpected_errors", "Number of unexpected errors while requesting headers");
    describe_histogram!("stages.headers.request_time", "Elapsed time of successful header requests");
}
