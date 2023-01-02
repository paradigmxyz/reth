extern crate metrics;
extern crate reth_metrics_derive;

use metrics::Gauge;
use reth_metrics_derive::Metrics;

fn main() {}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics {
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics2 {
    #[metric()]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics3 {
    #[metric(random = "value")]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics4 {
    #[metric(describe = 123)]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics5 {
    #[metric(rename = 123)]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics6 {
    #[metric(describe = "", describe = "")]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics7 {
    #[metric(rename = "_gauge", rename = "_gauge")]
    gauge: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "some_scope")]
struct CustomMetrics8 {
    #[metric(describe = "")]
    gauge: String,
}
