//! Prometheus exporter

use eyre::WrapErr;
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::layers::{PrefixLayer, Stack};
use std::net::SocketAddr;

pub(crate) fn initialize(listen_addr: SocketAddr) -> eyre::Result<()> {
    let (recorder, exporter) = PrometheusBuilder::new()
        .with_http_listener(listen_addr)
        .build()
        .wrap_err("Could not build Prometheus endpoint.")?;
    tokio::task::spawn(exporter);
    Stack::new(recorder)
        .push(PrefixLayer::new("reth"))
        .install()
        .wrap_err("Couldn't set metrics recorder.")?;

    Ok(())
}
