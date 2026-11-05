//! Differential tests against the pinned `metrics-exporter-prometheus` recorder.

use super::*;
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use metrics_exporter_prometheus as upstream;
use metrics_util::layers::{Layer, PrefixLayer};
use std::{num::NonZeroU32, sync::Arc, thread};

/// Records a workload covering every metric kind, label precedence, escaping and sanitization.
fn workload() {
    describe_counter!("requests", "Total requests");
    describe_gauge!("temperature", "Weird \"description\"\nwith a \\ backslash");
    describe_histogram!("latency_seconds", "Request latency");

    counter!("requests").increment(3);
    counter!("requests", "method" => "get", "status" => "200").increment(5);
    // Same key as above, label order does not matter.
    counter!("requests", "status" => "200", "method" => "get").increment(1);
    counter!("requests", "method" => "post").absolute(42);
    counter!("1weird-name.with*chars", "1bad:key" => "\"quoted\"\n\\back\\").increment(1);
    counter!("env_override", "env" => "key").increment(1);
    counter!("total_sum").increment(2);

    gauge!("temperature").set(21.5);
    gauge!("temperature", "room" => "a").increment(4.0);
    gauge!("temperature", "room" => "a").decrement(1.25);
    gauge!("negative").set(-3.0);
    gauge!("not_a_number").set(f64::NAN);
    gauge!("infinite").set(f64::INFINITY);

    for sample in [0.0, 0.001, 0.25, 1.0, 1.5, 3.0, 10.0, 1e9, -2.0, f64::INFINITY] {
        histogram!("latency_seconds").record(sample);
        histogram!("latency_seconds", "path" => "/").record(sample * 2.0);
    }
    for sample in 0..1000 {
        histogram!("payload_bytes", "kind" => "block").record(f64::from(sample));
    }
    histogram!("checksum").record(1.0);
    // Registered but never recorded.
    let _ = histogram!("idle_histogram");
}

/// Sorts samples within a metric family and sorts families, since upstream renders in hash order.
fn normalize(output: &str) -> Vec<Vec<String>> {
    let mut families = output
        .split("\n\n")
        .filter(|family| !family.is_empty())
        .map(|family| {
            let mut lines = family.lines().map(str::to_owned).collect::<Vec<_>>();
            lines.sort();
            lines
        })
        .collect::<Vec<_>>();
    families.sort();
    families
}

fn render_upstream(builder: upstream::PrometheusBuilder, prefix: Option<&'static str>) -> String {
    let recorder = builder.build_recorder();
    let handle = recorder.handle();
    match prefix {
        Some(prefix) => {
            let recorder = PrefixLayer::new(prefix).layer(recorder);
            metrics::with_local_recorder(&recorder, workload);
        }
        None => metrics::with_local_recorder(&recorder, workload),
    }
    handle.render()
}

fn render_local(builder: PrometheusBuilder, prefix: Option<&'static str>) -> String {
    let recorder = match prefix {
        Some(prefix) => builder.with_prefix(prefix).build_recorder(),
        None => builder.build_recorder(),
    };
    metrics::with_local_recorder(&recorder, workload);
    recorder.handle().render()
}

fn assert_same_output(
    upstream: upstream::PrometheusBuilder,
    local: PrometheusBuilder,
    prefix: Option<&'static str>,
) {
    let expected = render_upstream(upstream, prefix);
    let actual = render_local(local, prefix);
    assert_eq!(normalize(&actual), normalize(&expected), "local:\n{actual}\nupstream:\n{expected}");
}

#[test]
fn matches_upstream_defaults() {
    assert_same_output(upstream::PrometheusBuilder::new(), PrometheusBuilder::new(), None);
}

#[test]
fn matches_upstream_with_prefix() {
    assert_same_output(upstream::PrometheusBuilder::new(), PrometheusBuilder::new(), Some("reth"));
}

#[test]
fn matches_upstream_with_global_labels() {
    assert_same_output(
        upstream::PrometheusBuilder::new()
            .add_global_label("env", "global")
            .add_global_label("chain", "1")
            .add_global_label("env", "replaced"),
        PrometheusBuilder::new()
            .add_global_label("env", "global")
            .add_global_label("chain", "1")
            .add_global_label("env", "replaced"),
        Some("reth"),
    );
}

#[test]
fn matches_upstream_with_buckets() {
    let buckets = [0.005, 0.25, 1.0, 5.0, 100.0];
    assert_same_output(
        upstream::PrometheusBuilder::new().set_buckets(&buckets).unwrap(),
        PrometheusBuilder::new().set_buckets(&buckets).unwrap(),
        None,
    );
}

#[test]
fn matches_upstream_with_overrides() {
    let upstream = upstream::PrometheusBuilder::new()
        .set_quantiles(&[0.1, 0.5, 0.75, 2.0])
        .unwrap()
        .set_bucket_duration(Duration::from_secs(5))
        .unwrap()
        .set_bucket_count(NonZeroU32::new(4).unwrap())
        .set_buckets_for_metric(upstream::Matcher::Suffix("_seconds".to_string()), &[1.0, 2.0])
        .unwrap()
        .set_buckets_for_metric(upstream::Matcher::Full("reth.payload_bytes".to_string()), &[10.0])
        .unwrap();
    let local = PrometheusBuilder::new()
        .set_quantiles(&[0.1, 0.5, 0.75, 2.0])
        .unwrap()
        .set_bucket_duration(Duration::from_secs(5))
        .unwrap()
        .set_bucket_count(NonZeroU32::new(4).unwrap())
        .set_buckets_for_metric(Matcher::Suffix("_seconds".to_string()), &[1.0, 2.0])
        .unwrap()
        .set_buckets_for_metric(Matcher::Full("reth.payload_bytes".to_string()), &[10.0])
        .unwrap();
    assert_same_output(upstream, local, Some("reth"));
}

#[test]
fn matches_upstream_after_upkeep_and_repeated_renders() {
    let upstream_recorder = upstream::PrometheusBuilder::new().build_recorder();
    let upstream_handle = upstream_recorder.handle();
    let local_recorder = PrometheusBuilder::new().build_recorder();
    let local_handle = local_recorder.handle();

    for _ in 0..3 {
        metrics::with_local_recorder(&upstream_recorder, workload);
        metrics::with_local_recorder(&local_recorder, workload);
        upstream_handle.run_upkeep();
        local_handle.run_upkeep();
        assert_eq!(normalize(&local_handle.render()), normalize(&upstream_handle.render()));
    }
}

#[test]
fn first_description_wins() {
    let recorder = PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || {
        describe_gauge!("g", "first");
        describe_gauge!("g", "second");
        gauge!("g").set(1.0);
    });
    assert_eq!(recorder.handle().render(), "# HELP g first\n# TYPE g gauge\ng 1\n\n");
}

#[test]
fn rejects_invalid_builder_values() {
    assert!(matches!(
        PrometheusBuilder::new().set_quantiles(&[]),
        Err(BuildError::EmptyBucketsOrQuantiles)
    ));
    assert!(matches!(
        PrometheusBuilder::new().set_buckets(&[]),
        Err(BuildError::EmptyBucketsOrQuantiles)
    ));
    assert!(matches!(
        PrometheusBuilder::new().set_buckets_for_metric(Matcher::Full("a".to_string()), &[]),
        Err(BuildError::EmptyBucketsOrQuantiles)
    ));
    assert!(matches!(
        PrometheusBuilder::new().set_bucket_duration(Duration::ZERO),
        Err(BuildError::ZeroBucketDuration)
    ));
}

#[test]
fn render_is_sorted() {
    let recorder = PrometheusBuilder::new().build_recorder();
    metrics::with_local_recorder(&recorder, || {
        counter!("b").increment(1);
        counter!("a", "x" => "2").increment(1);
        counter!("a", "x" => "1").increment(1);
        gauge!("c").set(1.0);
    });
    assert_eq!(
        recorder.handle().render(),
        "# TYPE a counter\na{x=\"1\"} 1\na{x=\"2\"} 1\n\n# TYPE b counter\nb 1\n\n# TYPE c gauge\nc 1\n\n"
    );
}

#[test]
fn concurrent_record_and_render() {
    const THREADS: u64 = 8;
    const ITERATIONS: u64 = 10_000;

    let recorder = Arc::new(PrometheusBuilder::new().build_recorder());
    let handle = recorder.handle();
    let writers = (0..THREADS)
        .map(|thread| {
            let recorder = recorder.clone();
            thread::spawn(move || {
                metrics::with_local_recorder(&*recorder, || {
                    let shared = counter!("shared");
                    for i in 0..ITERATIONS {
                        shared.increment(1);
                        counter!("per_thread", "thread" => thread.to_string()).increment(1);
                        histogram!("samples").record(i as f64);
                    }
                })
            })
        })
        .collect::<Vec<_>>();

    while !writers.iter().all(|writer| writer.is_finished()) {
        handle.render();
        handle.run_upkeep();
    }
    for writer in writers {
        writer.join().unwrap();
    }

    // Histogram samples pushed while a render drains them can be lost, as upstream, so only
    // counters are exact here.
    let output = handle.render();
    assert!(output.contains(&format!("\nshared {}\n", THREADS * ITERATIONS)), "{output}");
    for thread in 0..THREADS {
        assert!(output.contains(&format!("per_thread{{thread=\"{thread}\"}} {ITERATIONS}\n")));
    }
}


