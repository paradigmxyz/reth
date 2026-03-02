use crate::{
    chain::ChainSpecInfo,
    hooks::{Hook, Hooks},
    recorder::install_prometheus_recorder,
    version::VersionInfo,
};
use bytes::Bytes;
use eyre::WrapErr;
use http::{header::CONTENT_TYPE, HeaderValue, Request, Response, StatusCode};
use http_body_util::Full;
use metrics::describe_gauge;
use metrics_process::Collector;
use reqwest::Client;
use reth_metrics::metrics::Unit;
use reth_tasks::TaskExecutor;
use std::{convert::Infallible, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

/// Configuration for the [`MetricServer`]
#[derive(Debug)]
pub struct MetricServerConfig {
    listen_addr: SocketAddr,
    version_info: VersionInfo,
    chain_spec_info: ChainSpecInfo,
    task_executor: TaskExecutor,
    hooks: Hooks,
    push_gateway_url: Option<String>,
    push_gateway_interval: Duration,
    pprof_dump_dir: PathBuf,
}

impl MetricServerConfig {
    /// Create a new [`MetricServerConfig`] with the given configuration
    pub const fn new(
        listen_addr: SocketAddr,
        version_info: VersionInfo,
        chain_spec_info: ChainSpecInfo,
        task_executor: TaskExecutor,
        hooks: Hooks,
        pprof_dump_dir: PathBuf,
    ) -> Self {
        Self {
            listen_addr,
            hooks,
            task_executor,
            version_info,
            chain_spec_info,
            push_gateway_url: None,
            push_gateway_interval: Duration::from_secs(5),
            pprof_dump_dir,
        }
    }

    /// Set the gateway URL and interval for pushing metrics
    pub fn with_push_gateway(mut self, url: Option<String>, interval: Duration) -> Self {
        self.push_gateway_url = url;
        self.push_gateway_interval = interval;
        self
    }
}

/// [`MetricServer`] responsible for serving the metrics endpoint
#[derive(Debug)]
pub struct MetricServer {
    config: MetricServerConfig,
}

impl MetricServer {
    /// Create a new [`MetricServer`] with the given configuration
    pub const fn new(config: MetricServerConfig) -> Self {
        Self { config }
    }

    /// Spawns the metrics server
    pub async fn serve(&self) -> eyre::Result<()> {
        let MetricServerConfig {
            listen_addr,
            hooks,
            task_executor,
            version_info,
            chain_spec_info,
            push_gateway_url,
            push_gateway_interval,
            pprof_dump_dir,
        } = &self.config;

        let hooks_for_endpoint = hooks.clone();
        self.start_endpoint(
            *listen_addr,
            Arc::new(move || hooks_for_endpoint.iter().for_each(|hook| hook())),
            task_executor.clone(),
            pprof_dump_dir.clone(),
        )
        .await
        .wrap_err_with(|| format!("Could not start Prometheus endpoint at {listen_addr}"))?;

        // Start push-gateway task if configured
        if let Some(url) = push_gateway_url {
            self.start_push_gateway_task(
                url.clone(),
                *push_gateway_interval,
                hooks.clone(),
                task_executor.clone(),
            )?;
        }

        // Describe metrics after recorder installation
        describe_db_metrics();
        describe_static_file_metrics();
        describe_rocksdb_metrics();
        Collector::default().describe();
        describe_memory_stats();
        describe_io_stats();

        version_info.register_version_metrics();
        chain_spec_info.register_chain_spec_metrics();

        Ok(())
    }

    async fn start_endpoint<F: Hook + 'static>(
        &self,
        listen_addr: SocketAddr,
        hook: Arc<F>,
        task_executor: TaskExecutor,
        pprof_dump_dir: PathBuf,
    ) -> eyre::Result<()> {
        let listener = tokio::net::TcpListener::bind(listen_addr)
            .await
            .wrap_err("Could not bind to address")?;

        tracing::info!(target: "reth::cli", "Starting metrics endpoint at {}", listener.local_addr().unwrap());

        task_executor.spawn_with_graceful_shutdown_signal(async move |mut signal| loop {
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

            let handle = install_prometheus_recorder();
            let hook = hook.clone();
            let pprof_dump_dir = pprof_dump_dir.clone();
            let service = tower::service_fn(move |req: Request<_>| {
                let response = handle_request(req.uri().path(), &*hook, handle, &pprof_dump_dir);
                async move { Ok::<_, Infallible>(response) }
            });

            let mut shutdown = signal.clone().ignore_guard();
            tokio::task::spawn(async move {
                let _ = jsonrpsee_server::serve_with_graceful_shutdown(io, service, &mut shutdown)
                    .await
                    .inspect_err(|error| tracing::debug!(%error, "failed to serve request"));
            });
        });

        Ok(())
    }

    /// Starts a background task to push metrics to a metrics gateway
    fn start_push_gateway_task(
        &self,
        url: String,
        interval: Duration,
        hooks: Hooks,
        task_executor: TaskExecutor,
    ) -> eyre::Result<()> {
        let client = Client::builder()
            .build()
            .wrap_err("Could not create HTTP client to push metrics to gateway")?;
        task_executor.spawn_with_graceful_shutdown_signal(async move |mut signal| {
            tracing::info!(url = %url, interval = ?interval, "Starting task to push metrics to gateway");
            let handle = install_prometheus_recorder();
            loop {
                tokio::select! {
                    _ = &mut signal => {
                        tracing::info!("Shutting down task to push metrics to gateway");
                        break;
                    }
                    _ = tokio::time::sleep(interval) => {
                        hooks.iter().for_each(|hook| hook());
                        let metrics = handle.handle().render();
                        match client.put(&url).header("Content-Type", "text/plain").body(metrics).send().await {
                            Ok(response) => {
                                if !response.status().is_success() {
                                    tracing::warn!(
                                        status = %response.status(),
                                        "Failed to push metrics to gateway"
                                    );
                                }
                            }
                            Err(err) => {
                                tracing::warn!(%err, "Failed to push metrics to gateway");
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }
}

fn describe_db_metrics() {
    describe_gauge!("db.table_size", Unit::Bytes, "The size of a database table (in bytes)");
    describe_gauge!("db.table_pages", "The number of database pages for a table");
    describe_gauge!("db.table_entries", "The number of entries for a table");
    describe_gauge!("db.freelist", "The number of pages on the freelist");
    describe_gauge!("db.page_size", Unit::Bytes, "The size of a database page (in bytes)");
    describe_gauge!(
        "db.timed_out_not_aborted_transactions",
        "Number of timed out transactions that were not aborted by the user yet"
    );
}

fn describe_static_file_metrics() {
    describe_gauge!("static_files.segment_size", Unit::Bytes, "The size of a static file segment");
    describe_gauge!("static_files.segment_files", "The number of files for a static file segment");
    describe_gauge!(
        "static_files.segment_entries",
        "The number of entries for a static file segment"
    );
}

fn describe_rocksdb_metrics() {
    describe_gauge!(
        "rocksdb.table_size",
        Unit::Bytes,
        "The estimated size of a RocksDB table (SST + memtable)"
    );
    describe_gauge!("rocksdb.table_entries", "The estimated number of keys in a RocksDB table");
    describe_gauge!(
        "rocksdb.pending_compaction_bytes",
        Unit::Bytes,
        "Bytes pending compaction for a RocksDB table"
    );
    describe_gauge!("rocksdb.sst_size", Unit::Bytes, "The size of SST files for a RocksDB table");
    describe_gauge!(
        "rocksdb.memtable_size",
        Unit::Bytes,
        "The size of memtables for a RocksDB table"
    );
    describe_gauge!(
        "rocksdb.wal_size",
        Unit::Bytes,
        "The total size of WAL (Write-Ahead Log) files. Important: this is not included in table_size or sst_size metrics"
    );
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
const fn describe_memory_stats() {}

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
const fn describe_io_stats() {}

fn handle_request(
    path: &str,
    hook: impl Fn(),
    handle: &crate::recorder::PrometheusRecorder,
    pprof_dump_dir: &PathBuf,
) -> Response<Full<Bytes>> {
    match path {
        "/debug/pprof/heap" => handle_pprof_heap(pprof_dump_dir),
        _ => {
            hook();
            let metrics = handle.handle().render();
            let mut response = Response::new(Full::new(Bytes::from(metrics)));
            response.headers_mut().insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
            response
        }
    }
}

#[cfg(all(feature = "jemalloc-prof", unix))]
fn handle_pprof_heap(pprof_dump_dir: &PathBuf) -> Response<Full<Bytes>> {
    use http::header::CONTENT_ENCODING;

    match jemalloc_pprof::PROF_CTL.as_ref() {
        Some(prof_ctl) => match prof_ctl.try_lock() {
            Ok(_) => match jemalloc_pprof_dump(pprof_dump_dir) {
                Ok(pprof) => {
                    let mut response = Response::new(Full::new(Bytes::from(pprof)));
                    response
                        .headers_mut()
                        .insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
                    response
                        .headers_mut()
                        .insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
                    response
                }
                Err(err) => {
                    let mut response = Response::new(Full::new(Bytes::from(format!(
                        "Failed to dump pprof: {err}"
                    ))));
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    response
                }
            },
            Err(_) => {
                let mut response = Response::new(Full::new(Bytes::from_static(
                    b"Profile dump already in progress. Try again later.",
                )));
                *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                response
            }
        },
        None => {
            let mut response = Response::new(Full::new(Bytes::from_static(
                b"jemalloc profiling not enabled. \
                 Set MALLOC_CONF=prof:true or rebuild with jemalloc-prof feature.",
            )));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        }
    }
}

/// Equivalent to [`jemalloc_pprof::JemallocProfCtl::dump`], but accepts a directory that the
/// temporary pprof file will be written to. The file is deleted when the function exits.
#[cfg(all(feature = "jemalloc-prof", unix))]
fn jemalloc_pprof_dump(pprof_dump_dir: &PathBuf) -> eyre::Result<Vec<u8>> {
    use std::{ffi::CString, io::BufReader};

    use mappings::MAPPINGS;
    use pprof_util::parse_jeheap;
    use tempfile::NamedTempFile;

    reth_fs_util::create_dir_all(pprof_dump_dir)?;
    let f = NamedTempFile::new_in(pprof_dump_dir)?;
    let path = CString::new(f.path().as_os_str().as_encoded_bytes()).unwrap();

    // SAFETY: "prof.dump" is documented as being writable and taking a C string as input:
    // http://jemalloc.net/jemalloc.3.html#prof.dump
    unsafe { tikv_jemalloc_ctl::raw::write(b"prof.dump\0", path.as_ptr()) }?;

    let dump_reader = BufReader::new(f);
    let profile =
        parse_jeheap(dump_reader, MAPPINGS.as_deref()).map_err(|err| eyre::eyre!(Box::new(err)))?;
    let pprof = profile.to_pprof(("inuse_space", "bytes"), ("space", "bytes"), None);

    Ok(pprof)
}

#[cfg(not(all(feature = "jemalloc-prof", unix)))]
fn handle_pprof_heap(_pprof_dump_dir: &PathBuf) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from_static(
        b"jemalloc pprof support not compiled. Rebuild with the jemalloc-prof feature.",
    )));
    *response.status_mut() = StatusCode::NOT_IMPLEMENTED;
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;
    use reth_tasks::Runtime;
    use socket2::{Domain, Socket, Type};
    use std::net::{SocketAddr, TcpListener};

    fn get_random_available_addr() -> SocketAddr {
        let addr = &"127.0.0.1:0".parse::<SocketAddr>().unwrap().into();
        let socket = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        socket.set_reuse_address(true).unwrap();
        socket.bind(addr).unwrap();
        socket.listen(1).unwrap();
        let listener = TcpListener::from(socket);
        listener.local_addr().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_metrics_endpoint() {
        let chain_spec_info = ChainSpecInfo { name: "test".to_string() };
        let version_info = VersionInfo {
            version: "test",
            build_timestamp: "test",
            cargo_features: "test",
            git_sha: "test",
            target_triple: "test",
            build_profile: "test",
        };

        let runtime = Runtime::test();

        let hooks = Hooks::builder().build();

        let listen_addr = get_random_available_addr();
        let config = MetricServerConfig::new(
            listen_addr,
            version_info,
            chain_spec_info,
            runtime.clone(),
            hooks,
            std::env::temp_dir(),
        );

        MetricServer::new(config).serve().await.unwrap();

        // Send request to the metrics endpoint
        let url = format!("http://{listen_addr}");
        let response = Client::new().get(&url).send().await.unwrap();
        assert!(response.status().is_success());

        // Check the response body
        let body = response.text().await.unwrap();
        assert!(body.contains("reth_process_cpu_seconds_total"));
        assert!(body.contains("reth_process_start_time_seconds"));

        // Make sure the runtime is dropped after the test runs.
        drop(runtime);
    }
}
