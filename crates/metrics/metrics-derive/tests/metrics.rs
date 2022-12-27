use std::{collections::HashMap, sync::Mutex};

use metrics::{
    set_recorder, Counter, Gauge, Histogram, Key, KeyName, Recorder, SharedString, Unit,
};
use once_cell::sync::Lazy;
use reth_metrics_derive::Metrics;
use serial_test::serial;

#[allow(dead_code)]
#[derive(Metrics)]
#[metrics(scope = "metrics_custom")]
struct CustomMetrics {
    /// A gauge with doc comment description.
    gauge: Gauge,
    #[metric(rename = "second_gauge", describe = "A gauge with metric attribute description.")]
    gauge2: Gauge,
    /// Some doc comment
    #[metric(describe = "Metric attribute description will be preffered over doc comment.")]
    counter: Counter,
    /// A renamed histogram.
    #[metric(rename = "histogram")]
    histo: Histogram,
}

static RECORDER: Lazy<TestRecorder> = Lazy::new(|| TestRecorder::new());

#[test]
#[serial]
fn describe_metrics() {
    let _ = set_recorder(&*RECORDER as &dyn Recorder); // ignore error

    CustomMetrics::describe();

    assert_eq!(RECORDER.metrics_len(), 4);

    let gauge = RECORDER.get_metric("metrics_custom_gauge");
    assert!(gauge.is_some());
    assert_eq!(
        gauge.unwrap(),
        TestMetric {
            ty: TestMetricTy::Gauge,
            description: Some("A gauge with doc comment description.".to_owned())
        }
    );

    let second_gauge = RECORDER.get_metric("metrics_custom_second_gauge");
    assert!(second_gauge.is_some());
    assert_eq!(
        second_gauge.unwrap(),
        TestMetric {
            ty: TestMetricTy::Gauge,
            description: Some("A gauge with metric attribute description.".to_owned())
        }
    );

    let counter = RECORDER.get_metric("metrics_custom_counter");
    assert!(counter.is_some());
    assert_eq!(
        counter.unwrap(),
        TestMetric {
            ty: TestMetricTy::Counter,
            description: Some(
                "Metric attribute description will be preffered over doc comment.".to_owned()
            )
        }
    );

    let histogram = RECORDER.get_metric("metrics_custom_histogram");
    assert!(histogram.is_some());
    assert_eq!(
        histogram.unwrap(),
        TestMetric {
            ty: TestMetricTy::Histogram,
            description: Some("A renamed histogram.".to_owned())
        }
    );

    RECORDER.clear();
}

#[test]
#[serial]
fn register_metrics() {
    let _ = set_recorder(&*RECORDER as &dyn Recorder); // ignore error

    let _metrics = CustomMetrics::default();

    assert_eq!(RECORDER.metrics_len(), 4);

    let gauge = RECORDER.get_metric("metrics_custom_gauge");
    assert!(gauge.is_some());
    assert_eq!(gauge.unwrap(), TestMetric { ty: TestMetricTy::Gauge, description: None });

    let second_gauge = RECORDER.get_metric("metrics_custom_second_gauge");
    assert!(second_gauge.is_some());
    assert_eq!(second_gauge.unwrap(), TestMetric { ty: TestMetricTy::Gauge, description: None });

    let counter = RECORDER.get_metric("metrics_custom_counter");
    assert!(counter.is_some());
    assert_eq!(counter.unwrap(), TestMetric { ty: TestMetricTy::Counter, description: None });

    let histogram = RECORDER.get_metric("metrics_custom_histogram");
    assert!(histogram.is_some());
    assert_eq!(histogram.unwrap(), TestMetric { ty: TestMetricTy::Histogram, description: None });

    RECORDER.clear();
}

struct TestRecorder {
    // Metrics map: key => Option<description>
    metrics: Mutex<HashMap<String, TestMetric>>,
}

#[derive(PartialEq, Clone, Debug)]
enum TestMetricTy {
    Counter,
    Gauge,
    Histogram,
}

#[derive(PartialEq, Clone, Debug)]
struct TestMetric {
    ty: TestMetricTy,
    description: Option<String>,
}

impl TestRecorder {
    fn new() -> Self {
        Self { metrics: Mutex::new(HashMap::default()) }
    }

    fn metrics_len(&self) -> usize {
        self.metrics.lock().expect("failed to lock metrics").len()
    }

    fn get_metric(&self, key: &str) -> Option<TestMetric> {
        self.metrics.lock().expect("failed to lock metrics").get(key).cloned()
    }

    fn record_metric(&self, key: &str, ty: TestMetricTy, description: Option<String>) {
        self.metrics
            .lock()
            .expect("failed to lock metrics")
            .insert(key.to_owned(), TestMetric { ty, description });
    }

    fn clear(&self) {
        self.metrics.lock().expect("failed to lock metrics").clear();
    }
}

impl Recorder for TestRecorder {
    fn describe_counter(&self, key: KeyName, _unit: Option<Unit>, description: SharedString) {
        self.record_metric(key.as_str(), TestMetricTy::Counter, Some(description.into_owned()))
    }

    fn describe_gauge(&self, key: KeyName, _unit: Option<Unit>, description: SharedString) {
        self.record_metric(key.as_str(), TestMetricTy::Gauge, Some(description.into_owned()))
    }

    fn describe_histogram(&self, key: KeyName, _unit: Option<Unit>, description: SharedString) {
        self.record_metric(key.as_str(), TestMetricTy::Histogram, Some(description.into_owned()))
    }

    fn register_counter(&self, key: &Key) -> Counter {
        self.record_metric(key.name(), TestMetricTy::Counter, None);
        Counter::noop()
    }

    fn register_gauge(&self, key: &Key) -> Gauge {
        self.record_metric(key.name(), TestMetricTy::Gauge, None);
        Gauge::noop()
    }

    fn register_histogram(&self, key: &Key) -> Histogram {
        self.record_metric(key.name(), TestMetricTy::Histogram, None);
        Histogram::noop()
    }
}
