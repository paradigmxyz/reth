//! Prometheus exporter

use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::layers::{PrefixLayer, Stack};

pub(crate) fn initialize_prometheus_exporter() {
    let (recorder, exporter) = PrometheusBuilder::new().build().expect("couldn't build Prometheus");
    tokio::task::spawn(exporter);
    Stack::new(recorder)
        .push(PrefixLayer::new("reth"))
        .install()
        .expect("couldn't install metrics recorder");
}
