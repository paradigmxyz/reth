use metrics::{describe_counter, describe_histogram};

/// Describe stagedsync headers metrics
fn describe_header_metrics() {
    describe_counter!("stages.headers.counter", "Number of headers successfully retrieved");
    describe_counter!(
        "stages.headers.timeout_errors",
        "Number of timeout errors while requesting headers"
    );
    describe_counter!(
        "stages.headers.validation_errors",
        "Number of validation errors while requesting headers"
    );
    describe_counter!(
        "stages.headers.unexpected_errors",
        "Number of unexpected errors while requesting headers"
    );
    describe_histogram!(
        "stages.headers.request_time",
        "Elapsed time of successful header requests"
    );
}

/// Describe stagedsync metrics
pub fn describe() {
    describe_header_metrics();
}
