use http_body_util::Full;
use hyper::{body::Bytes, server::conn::http1, service::service_fn, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::{
    collections::BTreeMap,
    convert::Infallible,
    sync::{Arc, Mutex},
};
use tokio::{net::TcpListener, task::JoinHandle};

/// Loopback snapshot server with optional byte-range support and a request log.
/// Each test owns its server, so ports and responses are isolated across parallel runs.
pub(super) struct SnapshotServer {
    pub(super) url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: JoinHandle<()>,
}

impl SnapshotServer {
    pub(super) async fn start(files: BTreeMap<String, Vec<u8>>, ranges: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let files = Arc::new(files);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let files = Arc::clone(&files);
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                        let path = request.uri().path().to_owned();
                        let range =
                            request.headers().get("range").map(|v| v.to_str().unwrap().to_owned());
                        log.lock().unwrap().push((path.clone(), range.clone()));
                        let mut response = Response::builder();
                        let body = if let Some(data) = files.get(&path) {
                            let mut bytes = data.as_slice();
                            if let Some(range) = range.filter(|_| ranges) {
                                let (start, end) =
                                    range.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
                                let start: usize = start.parse().unwrap();
                                let end = end
                                    .parse::<usize>()
                                    .unwrap_or(data.len() - 1)
                                    .min(data.len() - 1);
                                if start >= data.len() {
                                    response = response
                                        .status(StatusCode::RANGE_NOT_SATISFIABLE)
                                        .header("content-range", format!("bytes */{}", data.len()));
                                    bytes = &[];
                                } else {
                                    response = response.status(StatusCode::PARTIAL_CONTENT).header(
                                        "content-range",
                                        format!("bytes {start}-{end}/{}", data.len()),
                                    );
                                    bytes = &data[start..=end];
                                }
                            }
                            response = response.header("content-length", bytes.len());
                            if request.method() == hyper::Method::HEAD {
                                Bytes::new()
                            } else {
                                Bytes::copy_from_slice(bytes)
                            }
                        } else {
                            response = response.status(StatusCode::NOT_FOUND);
                            Bytes::new()
                        };
                        std::future::ready(Ok::<_, Infallible>(
                            response.body(Full::new(body)).unwrap(),
                        ))
                    });
                    // Range probes may close the connection without consuming the body.
                    let _ =
                        http1::Builder::new().serve_connection(TokioIo::new(stream), service).await;
                });
            }
        });
        Self { url, requests, task }
    }

    pub(super) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub(super) fn clear_requests(&self) {
        self.requests.lock().unwrap().clear();
    }
}

impl Drop for SnapshotServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type RecordedRequest = (String, Option<String>);
