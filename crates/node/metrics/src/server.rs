use crate::version::VersionInfo;
use eyre::WrapErr;
use http::{header::CONTENT_TYPE, HeaderValue, Response};
use metrics::describe_gauge;
use metrics_exporter_prometheus::PrometheusHandle;
use reth_db_api::database_metrics::DatabaseMetrics;
use reth_metrics::metrics::Unit;
use reth_provider::providers::StaticFileProvider;
use reth_tasks::TaskExecutor;
use std::{convert::Infallible, net::SocketAddr, sync::Arc};
use tracing::info;

pub(crate) trait Hook: Fn() + Send + Sync {}
impl<T: Fn() + Send + Sync> Hook for T {}

/// Configuration for the [`MetricServer`]
pub struct MetricServerConfig<Metrics>
where
    Metrics: DatabaseMetrics + 'static + Send + Sync,
{
    listen_addr: SocketAddr,
    prometheus_handle: PrometheusHandle,
    hooks: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
    task_executor: TaskExecutor,
    version_info: VersionInfo,
    collector: metrics_process::Collector,
    _phantom: std::marker::PhantomData<Metrics>,
}

impl<Metrics> MetricServerConfig<Metrics>
where
    Metrics: DatabaseMetrics + 'static + Send + Sync,
{
    /// Create a new [`MetricServerConfig`] with the given configuration
    pub fn new(
        listen_addr: SocketAddr,
        prometheus_handle: PrometheusHandle,
        db: Metrics,
        static_file_provider: StaticFileProvider,
        collector: metrics_process::Collector,
        task_executor: TaskExecutor,
        version_info: VersionInfo,
    ) -> Self {
        let cloned_c = collector.clone();
        let hooks: Vec<Box<dyn Hook<Output = ()>>> = vec![
            Box::new(move || db.report_metrics()),
            Box::new(move || {
                let _ = static_file_provider.report_metrics().map_err(
                    |error| tracing::error!(%error, "Failed to report static file provider metrics"),
                );
            }),
            Box::new(move || cloned_c.collect()),
            Box::new(collect_memory_stats),
            Box::new(collect_io_stats),
        ];
        Self {
            listen_addr,
            prometheus_handle,
            hooks: Arc::new(hooks),
            task_executor,
            version_info,
            collector,
            _phantom: std::marker::PhantomData,
        }
    }
}

/// [`MetricServer`] responsible for serving the metrics endpoint
pub struct MetricServer<Metrics>
where
    Metrics: DatabaseMetrics + 'static + Send + Sync,
{
    config: MetricServerConfig<Metrics>,
}

impl<Metrics> MetricServer<Metrics>
where
    Metrics: DatabaseMetrics + 'static + Send + Sync,
{
    /// Create a new [`MetricServer`] with the given configuration
    pub const fn new(config: MetricServerConfig<Metrics>) -> Self {
        Self { config }
    }

    /// Start the metrics server
    pub async fn serve(&self) -> eyre::Result<()> {
        let MetricServerConfig {
            listen_addr,
            prometheus_handle,
            hooks,
            task_executor,
            version_info,
            collector,
            ..
        } = &self.config;

        info!(target: "reth::cli", addr = %listen_addr, "Starting metrics endpoint");

        self.serve_with_hooks(
            *listen_addr,
            prometheus_handle.clone(),
            hooks.clone(),
            task_executor.clone(),
        )
        .await?;

        // Describe metrics after recorder installation
        describe_gauge!("db.table_size", Unit::Bytes, "The size of a database table (in bytes)");
        describe_gauge!("db.table_pages", "The number of database pages for a table");
        describe_gauge!("db.table_entries", "The number of entries for a table");
        describe_gauge!("db.freelist", "The number of pages on the freelist");
        describe_gauge!("db.page_size", Unit::Bytes, "The size of a database page (in bytes)");
        describe_gauge!(
            "db.timed_out_not_aborted_transactions",
            "Number of timed out transactions that were not aborted by the user yet"
        );

        describe_gauge!(
            "static_files.segment_size",
            Unit::Bytes,
            "The size of a static file segment"
        );
        describe_gauge!(
            "static_files.segment_files",
            "The number of files for a static file segment"
        );
        describe_gauge!(
            "static_files.segment_entries",
            "The number of entries for a static file segment"
        );

        collector.describe();
        describe_memory_stats();
        describe_io_stats();
        version_info.register_version_metrics();

        Ok(())
    }

    async fn serve_with_hooks<F: Hook + 'static>(
        &self,
        listen_addr: SocketAddr,
        handle: PrometheusHandle,
        hooks: Arc<Vec<F>>,
        task_executor: TaskExecutor,
    ) -> eyre::Result<()> {
        // Start endpoint
        self.start_endpoint(
            listen_addr,
            handle,
            Arc::new(move || hooks.iter().for_each(|hook| hook())),
            task_executor,
        )
        .await
        .wrap_err("Could not start Prometheus endpoint")?;

        Ok(())
    }

    async fn start_endpoint<F: Hook + 'static>(
        &self,
        listen_addr: SocketAddr,
        handle: PrometheusHandle,
        hook: Arc<F>,
        task_executor: TaskExecutor,
    ) -> eyre::Result<()> {
        let listener = tokio::net::TcpListener::bind(listen_addr)
            .await
            .wrap_err("Could not bind to address")?;

        task_executor.spawn_with_graceful_shutdown_signal(|mut signal| async move {
            loop {
                let io = tokio::select! {
                    _ = &mut signal => break,
                    io = listener.accept() => {
                        match io {
                            Ok((stream, _remote_addr)) => stream,
                            Err(err) => {
                                tracing::error!(%err, "failed to accept connection");
                                continue;
                            }
                        }
                    }
                };

                let handle = handle.clone();
                let hook = hook.clone();
                let service = tower::service_fn(move |_| {
                    (hook)();
                    let metrics = handle.render();
                    let mut response = Response::new(metrics);
                    response
                        .headers_mut()
                        .insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
                    async move { Ok::<_, Infallible>(response) }
                });

                let mut shutdown = signal.clone().ignore_guard();
                tokio::task::spawn(async move {
                    if let Err(error) =
                        jsonrpsee::server::serve_with_graceful_shutdown(io, service, &mut shutdown)
                            .await
                    {
                        tracing::debug!(%error, "failed to serve request")
                    }
                });
            }
        });

        Ok(())
    }
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
        gauge!("jemalloc.active").set(value as f64);
    }

    if let Ok(value) = stats::allocated::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.allocated"))
    {
        gauge!("jemalloc.allocated").set(value as f64);
    }

    if let Ok(value) = stats::mapped::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.mapped"))
    {
        gauge!("jemalloc.mapped").set(value as f64);
    }

    if let Ok(value) = stats::metadata::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.metadata"))
    {
        gauge!("jemalloc.metadata").set(value as f64);
    }

    if let Ok(value) = stats::resident::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.resident"))
    {
        gauge!("jemalloc.resident").set(value as f64);
    }

    if let Ok(value) = stats::retained::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.retained"))
    {
        gauge!("jemalloc.retained").set(value as f64);
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
const fn collect_memory_stats() {}

#[cfg(not(all(feature = "jemalloc", unix)))]
const fn describe_memory_stats() {}

#[cfg(target_os = "linux")]
fn collect_io_stats() {
    use metrics::counter;
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

    counter!("io.rchar").absolute(io.rchar);
    counter!("io.wchar").absolute(io.wchar);
    counter!("io.syscr").absolute(io.syscr);
    counter!("io.syscw").absolute(io.syscw);
    counter!("io.read_bytes").absolute(io.read_bytes);
    counter!("io.write_bytes").absolute(io.write_bytes);
    counter!("io.cancelled_write_bytes").absolute(io.cancelled_write_bytes);
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
const fn collect_io_stats() {}

#[cfg(not(target_os = "linux"))]
const fn describe_io_stats() {}
