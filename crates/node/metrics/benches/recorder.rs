#![allow(missing_docs)]

//! Compares the local Prometheus recorder with `metrics-exporter-prometheus` wrapped in the
//! `reth` prefix layer, as reth installed it.

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use metrics::{counter, gauge, Key, Label, Recorder};
use metrics_util::layers::{Layer, PrefixLayer};
use reth_node_metrics::PrometheusBuilder;
use std::{
    hint::black_box,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

/// Series counts of a `reth node --dev` scrape: ~1.3k counter and gauge series and ~720 summary
/// series over ~570 metric families.
const COUNTERS: usize = 700;
const GAUGES: usize = 600;
const HISTOGRAMS: usize = 720;
/// Samples recorded into every histogram between two scrapes.
const SAMPLES_PER_SCRAPE: usize = 200;

static METADATA: metrics::Metadata<'static> =
    metrics::Metadata::new(module_path!(), metrics::Level::INFO, Some(module_path!()));

struct Target {
    name: &'static str,
    recorder: Arc<dyn Recorder + Send + Sync>,
    render: Arc<dyn Fn() -> String + Send + Sync>,
    upkeep: Arc<dyn Fn() + Send + Sync>,
}

fn targets() -> [Target; 2] {
    let upstream = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let (upstream_render, upstream_upkeep) = (upstream.handle(), upstream.handle());
    let local = PrometheusBuilder::new().with_prefix("reth").build_recorder();
    let (local_render, local_upkeep) = (local.handle(), local.handle());
    [
        Target {
            name: "upstream",
            recorder: Arc::new(PrefixLayer::new("reth").layer(upstream)),
            render: Arc::new(move || upstream_render.render()),
            upkeep: Arc::new(move || upstream_upkeep.run_upkeep()),
        },
        Target {
            name: "local",
            recorder: Arc::new(local),
            render: Arc::new(move || local_render.render()),
            upkeep: Arc::new(move || local_upkeep.run_upkeep()),
        },
    ]
}

/// Deterministic log-uniform samples between 1µs and 10s, like reth's duration histograms.
struct Samples(u64);

impl Samples {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        let unit = (self.0 >> 11) as f64 / (1u64 << 53) as f64;
        1e-6 * 1e7f64.powf(unit)
    }
}

fn record(c: &mut Criterion) {
    let mut group = c.benchmark_group("record");
    for target in targets() {
        let recorder = &*target.recorder;
        let key = Key::from_static_name("sync.execution.gas_processed");
        let counter = recorder.register_counter(&key, &METADATA);
        group.bench_function(format!("{}/counter_cached", target.name), |b| {
            b.iter(|| counter.increment(black_box(1)))
        });

        let gauge = recorder.register_gauge(&key, &METADATA);
        group.bench_function(format!("{}/gauge_cached", target.name), |b| {
            b.iter(|| gauge.set(black_box(1.5)))
        });

        let histogram = recorder.register_histogram(&key, &METADATA);
        let upkeep = target.upkeep.clone();
        group.bench_function(format!("{}/histogram_cached", target.name), |b| {
            // Drain outside the timed section so samples don't pile up across iterations.
            b.iter_custom(|iters| {
                let mut elapsed = Duration::ZERO;
                let mut remaining = iters;
                while remaining > 0 {
                    let chunk = remaining.min(10_000);
                    let start = Instant::now();
                    for _ in 0..chunk {
                        histogram.record(black_box(0.25));
                    }
                    elapsed += start.elapsed();
                    upkeep();
                    remaining -= chunk;
                }
                elapsed
            })
        });

        // Lookup of an existing series through the macros, as scrape hooks and dynamically
        // labeled metrics do.
        metrics::with_local_recorder(recorder, || {
            let table = String::from("HashedStorages");
            group.bench_function(format!("{}/gauge_lookup_dynamic_label", target.name), |b| {
                b.iter(|| gauge!("db.table_size", "table" => table.clone()).set(black_box(1.0)))
            });
            group.bench_function(format!("{}/counter_lookup_static", target.name), |b| {
                b.iter(|| counter!("network.messages").increment(black_box(1)))
            });
        });

        let histogram = recorder.register_histogram(&key, &METADATA);
        let upkeep = target.upkeep.clone();
        group.bench_function(format!("{}/histogram_contended_8_threads", target.name), |b| {
            b.iter_custom(|iters| {
                let threads = 8;
                let per_thread = iters.div_ceil(threads);
                let barrier = Arc::new(Barrier::new(threads as usize + 1));
                let handles = (0..threads)
                    .map(|_| {
                        let (histogram, barrier) = (histogram.clone(), barrier.clone());
                        thread::spawn(move || {
                            barrier.wait();
                            for _ in 0..per_thread {
                                histogram.record(black_box(0.25));
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                barrier.wait();
                let start = Instant::now();
                for handle in handles {
                    handle.join().unwrap();
                }
                let elapsed = start.elapsed();
                upkeep();
                elapsed
            })
        });
    }
    group.finish();
}

/// Registers the dev node shape and returns the histogram handles.
fn populate(recorder: &dyn Recorder) -> Vec<metrics::Histogram> {
    for i in 0..COUNTERS {
        let key = Key::from_parts(
            format!("component{}.counter{i}", i % 40),
            vec![Label::new("kind", "a")],
        );
        recorder.register_counter(&key, &METADATA).increment(i as u64);
        recorder.describe_counter(key.name().to_string().into(), None, "A counter".into());
    }
    for i in 0..GAUGES {
        let key = Key::from_name(format!("component{}.gauge{i}", i % 40));
        recorder.register_gauge(&key, &METADATA).set(i as f64);
    }
    let mut samples = Samples(0x9e37_79b9_7f4a_7c15);
    (0..HISTOGRAMS)
        .map(|i| {
            let key = Key::from_name(format!("component{}.duration{i}", i % 40));
            let histogram = recorder.register_histogram(&key, &METADATA);
            for _ in 0..SAMPLES_PER_SCRAPE {
                histogram.record(samples.next());
            }
            histogram
        })
        .collect()
}

fn render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render");
    group.sample_size(30);
    for target in targets() {
        let histograms = populate(&*target.recorder);
        let mut samples = Samples(0x2545_f491_4f6c_dd1d);
        // Steady state: summaries hold samples from earlier scrapes.
        (target.render)();
        let render = target.render.clone();
        group.bench_function(format!("{}/dev_node_scrape", target.name), |b| {
            b.iter_batched(
                || {
                    for histogram in &histograms {
                        for _ in 0..SAMPLES_PER_SCRAPE {
                            histogram.record(samples.next());
                        }
                    }
                },
                |()| black_box(render()),
                BatchSize::PerIteration,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, record, render);
criterion_main!(benches);
