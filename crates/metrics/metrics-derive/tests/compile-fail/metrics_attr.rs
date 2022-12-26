extern crate reth_metrics_derive;
use reth_metrics_derive::Metrics;

fn main() {}

#[derive(Metrics)]
struct CustomMetrics;

#[derive(Metrics)]
#[metrics()]
#[metrics()]
struct CustomMetrics2;
