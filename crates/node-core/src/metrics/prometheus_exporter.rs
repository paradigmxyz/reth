//! Prometheus exporter

use crate::metrics::version_metrics::register_version_metrics;
use eyre::WrapErr;
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use metrics::describe_gauge;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::layers::{PrefixLayer, Stack};
use reth_db::database_metrics::DatabaseMetrics;
use reth_metrics::metrics::Unit;
use reth_provider::providers::StaticFileProvider;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};

pub(crate) trait Hook: Fn() + Send + Sync {}
impl<T: Fn() + Send + Sync> Hook for T {}

/// Installs Prometheus as the metrics recorder.
pub fn install_recorder() -> eyre::Result<PrometheusHandle> {
    let recorder = PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();

    // Build metrics stack
    Stack::new(recorder)
        .push(PrefixLayer::new("reth"))
        .install()
        .wrap_err("Couldn't set metrics recorder.")?;

    Ok(handle)
}

/// Serves Prometheus metrics over HTTP with hooks.
///
/// The hooks are called every time the metrics are requested at the given endpoint, and can be used
/// to record values for pull-style metrics, i.e. metrics that are not automatically updated.
pub(crate) async fn serve_with_hooks<F: Hook + 'static>(
    listen_addr: SocketAddr,
    handle: PrometheusHandle,
    hooks: impl IntoIterator<Item = F>,
) -> eyre::Result<()> {
    let hooks: Vec<_> = hooks.into_iter().collect();

    // Start endpoint
    start_endpoint(listen_addr, handle, Arc::new(move || hooks.iter().for_each(|hook| hook())))
        .await
        .wrap_err("Could not start Prometheus endpoint")?;

    Ok(())
}

/// Starts an endpoint at the given address to serve Prometheus metrics.
async fn start_endpoint<F: Hook + 'static>(
    listen_addr: SocketAddr,
    handle: PrometheusHandle,
    hook: Arc<F>,
) -> eyre::Result<()> {
    let make_svc = make_service_fn(move |_| {
        let handle = handle.clone();
        let hook = Arc::clone(&hook);
        async move {
            Ok::<_, Infallible>(service_fn(move |_: Request<Body>| {
                (hook)();
                let metrics = handle.render();
                async move { Ok::<_, Infallible>(Response::new(Body::from(metrics))) }
            }))
        }
    });
    let server =
        Server::try_bind(&listen_addr).wrap_err("Could not bind to address")?.serve(make_svc);

    tokio::spawn(async move { server.await.expect("Metrics endpoint crashed") });

    Ok(())
}

/// Serves Prometheus metrics over HTTP with database and process metrics.
pub async fn serve<Metrics>(
    listen_addr: SocketAddr,
    handle: PrometheusHandle,
    db: Metrics,
    static_file_provider: StaticFileProvider,
    process: metrics_process::Collector,
) -> eyre::Result<()>
where
    Metrics: DatabaseMetrics + 'static + Send + Sync,
{
    let db_metrics_hook = move || db.report_metrics();
    let static_file_metrics_hook = move || {
        let _ = static_file_provider.report_metrics().map_err(
            |error| tracing::error!(%error, "Failed to report static file provider metrics"),
        );
    };

    // Clone `process` to move it into the hook and use the original `process` for describe below.
    let cloned_process = process.clone();
    let hooks: Vec<Box<dyn Hook<Output = ()>>> = vec![
        Box::new(db_metrics_hook),
        Box::new(static_file_metrics_hook),
        Box::new(move || cloned_process.collect()),
        Box::new(collect_memory_stats),
        Box::new(collect_io_stats),
    ];
    serve_with_hooks(listen_addr, handle, hooks).await?;

    // We describe the metrics after the recorder is installed, otherwise this information is not
    // registered
    describe_gauge!("db.table_size", Unit::Bytes, "The size of a database table (in bytes)");
    describe_gauge!("db.table_pages", "The number of database pages for a table");
    describe_gauge!("db.table_entries", "The number of entries for a table");
    describe_gauge!("db.freelist", "The number of pages on the freelist");
    describe_gauge!(
        "db.timed_out_not_aborted_transactions",
        "Number of timed out transactions that were not aborted by the user yet"
    );

    describe_gauge!("static_files.segment_size", Unit::Bytes, "The size of a static file segment");
    describe_gauge!("static_files.segment_files", "The number of files for a static file segment");
    describe_gauge!(
        "static_files.segment_entries",
        "The number of entries for a static file segment"
    );

    process.describe();
    describe_memory_stats();
    describe_io_stats();
    register_version_metrics();

    Ok(())
}

#[cfg(all(feature = "jemalloc", unix))]
fn collect_memory_stats() {
    use metrics::gauge;
    use tikv_jemalloc_ctl::{epoch, stats};
    use tracing::error;

    if epoch::advance().map_err(|error| error!(%error, "Failed to advance jemalloc epoch")).is_err()
    {
        return
    }

    if let Ok(value) = stats::active::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.active"))
    {
        gauge!("jemalloc.active", value as f64);
    }

    if let Ok(value) = stats::allocated::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.allocated"))
    {
        gauge!("jemalloc.allocated", value as f64);
    }

    if let Ok(value) = stats::mapped::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.mapped"))
    {
        gauge!("jemalloc.mapped", value as f64);
    }

    if let Ok(value) = stats::metadata::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.metadata"))
    {
        gauge!("jemalloc.metadata", value as f64);
    }

    if let Ok(value) = stats::resident::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.resident"))
    {
        gauge!("jemalloc.resident", value as f64);
    }

    if let Ok(value) = stats::retained::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.retained"))
    {
        gauge!("jemalloc.retained", value as f64);
    }
}

#[cfg(all(feature = "jemalloc", unix))]
fn describe_memory_stats() {
    describe_gauge!(
        "jemalloc.active",
        Unit::Bytes,
        "Total number of bytes in active pages allocated by the application"
    );
    describe_gauge!(
        "jemalloc.allocated",
        Unit::Bytes,
        "Total number of bytes allocated by the application"
    );
    describe_gauge!(
        "jemalloc.mapped",
        Unit::Bytes,
        "Total number of bytes in active extents mapped by the allocator"
    );
    describe_gauge!(
        "jemalloc.metadata",
        Unit::Bytes,
        "Total number of bytes dedicated to jemalloc metadata"
    );
    describe_gauge!(
        "jemalloc.resident",
        Unit::Bytes,
        "Total number of bytes in physically resident data pages mapped by the allocator"
    );
    describe_gauge!(
        "jemalloc.retained",
        Unit::Bytes,
        "Total number of bytes in virtual memory mappings that were retained rather than \
        being returned to the operating system via e.g. munmap(2)"
    );
}

#[cfg(not(all(feature = "jemalloc", unix)))]
fn collect_memory_stats() {}

#[cfg(not(all(feature = "jemalloc", unix)))]
fn describe_memory_stats() {}

#[cfg(target_os = "linux")]
fn collect_io_stats() {
    use metrics::absolute_counter;
    use tracing::error;

    let Ok(process) = procfs::process::Process::myself()
        .map_err(|error| error!(%error, "Failed to get currently running process"))
    else {
        return
    };

    let Ok(io) = process.io().map_err(
        |error| error!(%error, "Failed to get IO stats for the currently running process"),
    ) else {
        return
    };

    absolute_counter!("io.rchar", io.rchar);
    absolute_counter!("io.wchar", io.wchar);
    absolute_counter!("io.syscr", io.syscr);
    absolute_counter!("io.syscw", io.syscw);
    absolute_counter!("io.read_bytes", io.read_bytes);
    absolute_counter!("io.write_bytes", io.write_bytes);
    absolute_counter!("io.cancelled_write_bytes", io.cancelled_write_bytes);
}

#[cfg(target_os = "linux")]
fn describe_io_stats() {
    use metrics::describe_counter;

    describe_counter!("io.rchar", "Characters read");
    describe_counter!("io.wchar", "Characters written");
    describe_counter!("io.syscr", "Read syscalls");
    describe_counter!("io.syscw", "Write syscalls");
    describe_counter!("io.read_bytes", Unit::Bytes, "Bytes read");
    describe_counter!("io.write_bytes", Unit::Bytes, "Bytes written");
    describe_counter!("io.cancelled_write_bytes", Unit::Bytes, "Cancelled write bytes");
}

#[cfg(not(target_os = "linux"))]
fn collect_io_stats() {}

#[cfg(not(target_os = "linux"))]
fn describe_io_stats() {}

#[cfg(test)]
mod tests {
    use crate::node_config::PROMETHEUS_RECORDER_HANDLE;
    use std::ops::Deref;

    // Dependencies using different version of the `metrics` crate (to be exact, 0.21 vs 0.22)
    // may not be able to communicate with each other through the global recorder.
    //
    // This test ensures that `metrics-process` dependency plays well with the current
    // `metrics-exporter-prometheus` dependency version.
    #[test]
    fn process_metrics() {
        // initialize the lazy handle
        let _ = PROMETHEUS_RECORDER_HANDLE.deref();

        let process = metrics_process::Collector::default();
        process.describe();
        process.collect();

        let metrics = PROMETHEUS_RECORDER_HANDLE.render();
        assert!(metrics.contains("process_cpu_seconds_total"));
    }
}
