extern crate reth_metrics_derive;
use reth_metrics_derive::Metrics;

fn main() {}

#[derive(Metrics)]
#[metrics()]
struct CustomMetrics;

#[derive(Metrics)]
#[metrics(scope = value)]
struct CustomMetrics2;

#[derive(Metrics)]
#[metrics(scope = 123)]
struct CustomMetrics3;

#[derive(Metrics)]
#[metrics(scope = "some.scope", scope = "another.scope")]
struct CustomMetrics4;

#[derive(Metrics)]
#[metrics(random = "value")]
struct CustomMetrics5;
