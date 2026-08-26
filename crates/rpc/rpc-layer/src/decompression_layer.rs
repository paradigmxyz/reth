use http::StatusCode;
use http_body_util::{BodyExt, LengthLimitError, Limited};
use jsonrpsee_http_client::{HttpBody, HttpRequest, HttpResponse};
use std::{
    future::{poll_fn, Future},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::Semaphore;
use tower::{Layer, Service};
use tower_http::decompression::{
    RequestDecompression, RequestDecompressionLayer as TowerDecompressionLayer,
};
use tracing::debug;

/// Maximum number of request bodies that may be decompressed concurrently.
///
/// Each in-flight decompression can materialize up to the configured maximum body size, so this
/// bounds the worst-case memory usage at `MAX_CONCURRENT_DECOMPRESSIONS * max_body_size` (120 MiB
/// with the default 15 MiB request size limit). Excess requests wait for a permit instead of
/// being rejected.
const MAX_CONCURRENT_DECOMPRESSIONS: usize = 8;

/// This layer is a wrapper around [`tower_http::decompression::RequestDecompressionLayer`] that
/// integrates with jsonrpsee's HTTP types.
#[expect(missing_debug_implementations)]
#[derive(Clone)]
pub struct DecompressionLayer {
    inner_layer: TowerDecompressionLayer,
    /// Maximum size in bytes for both compressed and decompressed bodies.
    max_body_size: usize,
    /// Bounds concurrent decompression work across all services created from this layer.
    decompression_permits: Arc<Semaphore>,
}

impl DecompressionLayer {
    /// Creates a new decompression layer from a list of algorithm names.
    /// Supported: zstd, gzip, deflate, br
    pub fn new(algos: &[impl AsRef<str>], max_body_size: usize) -> Self {
        // Start with all algorithms explicitly disabled
        let mut layer = TowerDecompressionLayer::new().no_zstd().no_gzip().no_deflate().no_br();

        // Only enable the algorithms that were explicitly passed.
        for algo in algos {
            match algo.as_ref() {
                "zstd" => layer = layer.zstd(true),
                "gzip" => layer = layer.gzip(true),
                "deflate" => layer = layer.deflate(true),
                "br" | "brotli" => layer = layer.br(true),
                _ => {}
            }
        }

        Self {
            inner_layer: layer,
            max_body_size,
            decompression_permits: Arc::new(Semaphore::new(MAX_CONCURRENT_DECOMPRESSIONS)),
        }
    }
}

impl<S> Layer<S> for DecompressionLayer {
    type Service = DecompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DecompressionService {
            decompression: self.inner_layer.layer(InnerService {
                inner,
                max_body_size: self.max_body_size,
                decompression_permits: self.decompression_permits.clone(),
            }),
            max_body_size: self.max_body_size,
        }
    }
}

/// Service that performs request decompression with body size limiting.
///
/// Created by [`DecompressionLayer`].
#[expect(missing_debug_implementations)]
#[derive(Clone)]
pub struct DecompressionService<S> {
    decompression: RequestDecompression<InnerService<S>>,
    max_body_size: usize,
}

/// Inner service wrapper to handle type conversion between jsonrpsee and `tower_http`
/// with body size limiting.
#[derive(Clone)]
struct InnerService<S> {
    inner: S,
    max_body_size: usize,
    decompression_permits: Arc<Semaphore>,
}

/// Marks a request whose body Tower will decompress before it reaches [`InnerService`].
#[derive(Clone, Copy, Debug)]
struct CompressedBody {
    content_length: Option<u64>,
}

impl<S> Service<http::Request<tower_http::decompression::DecompressionBody<HttpBody>>>
    for InnerService<S>
where
    S: Service<HttpRequest, Response = HttpResponse> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = http::Response<HttpBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(
        &mut self,
        req: http::Request<tower_http::decompression::DecompressionBody<HttpBody>>,
    ) -> Self::Future {
        let mut inner = self.inner.clone();
        let max_body_size = self.max_body_size;
        let decompression_permits = self.decompression_permits.clone();
        Box::pin(async move {
            let (mut parts, body) = req.into_parts();

            let Some(compressed) = parts.extensions.remove::<CompressedBody>() else {
                poll_fn(|cx| inner.poll_ready(cx)).await?;
                return inner.call(HttpRequest::from_parts(parts, HttpBody::new(body))).await;
            };

            if compressed.content_length.is_some_and(|length| length > max_body_size as u64) {
                return Ok(err_response(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"));
            }

            // HTTP/2 allows many concurrent streams per connection, so bound how many bodies are
            // decompressed and materialized at once to cap memory and CPU usage.
            let permit = decompression_permits
                .acquire_owned()
                .await
                .expect("decompression semaphore is never closed");

            let body = match Limited::new(body, max_body_size).collect().await {
                Ok(body) => body,
                Err(err) if err.is::<LengthLimitError>() => {
                    return Ok(err_response(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large"));
                }
                Err(err) => {
                    debug!(target: "rpc::decompression", %err, "Failed to decompress request body");
                    return Ok(err_response(StatusCode::BAD_REQUEST, "Invalid compressed body"));
                }
            };

            // Decompression is done; release the permit before dispatching the request.
            drop(permit);

            poll_fn(|cx| inner.poll_ready(cx)).await?;
            inner.call(HttpRequest::from_parts(parts, HttpBody::new(body))).await
        })
    }
}

impl<S> Service<HttpRequest> for DecompressionService<S>
where
    S: Service<HttpRequest, Response = HttpResponse> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.decompression.poll_ready(cx)
    }

    fn call(&mut self, mut req: HttpRequest) -> Self::Future {
        // RFC 9110 §8.4.1: content-coding tokens are case-insensitive, but `tower_http` matches
        // lowercase bytes exactly, so normalize the header before decompression.
        if let Some(encoding) = req.headers().get(http::header::CONTENT_ENCODING) &&
            encoding.as_bytes().iter().any(u8::is_ascii_uppercase) &&
            let Ok(normalized) =
                http::HeaderValue::from_bytes(&encoding.as_bytes().to_ascii_lowercase())
        {
            req.headers_mut().insert(http::header::CONTENT_ENCODING, normalized);
        }

        if req
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .is_some_and(|encoding| encoding.as_bytes() != b"identity")
        {
            let content_length = req
                .headers()
                .get(http::header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok());
            req.extensions_mut().insert(CompressedBody { content_length });

            let (parts, body) = req.into_parts();
            req = HttpRequest::from_parts(
                parts,
                HttpBody::new(Limited::new(body, self.max_body_size)),
            );
        }

        let fut = self.decompression.call(req);

        Box::pin(async move { Ok(fut.await?.map(HttpBody::new)) })
    }
}

#[inline]
fn err_response(status: StatusCode, msg: &'static str) -> HttpResponse {
    http::Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(HttpBody::from(msg))
        .expect("static error response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::CONTENT_ENCODING;
    use http_body_util::BodyExt;
    use jsonrpsee_http_client::{HttpRequest, HttpResponse};
    use std::{convert::Infallible, future::ready, io::Write};

    const TEST_DATA: &str = r#"{"method":"test","params":["test data"],"id":1}"#;
    const DEFAULT_MAX_SIZE: usize = 15 * 1024 * 1024;

    type Compressor = fn(&[u8]) -> Vec<u8>;

    #[derive(Clone)]
    struct MockEchoService;

    impl Service<HttpRequest> for MockEchoService {
        type Response = HttpResponse;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(
            &mut self,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: HttpRequest) -> Self::Future {
            let (_parts, body) = req.into_parts();
            ready(Ok(HttpResponse::builder().status(200).body(body).unwrap()))
        }
    }

    fn setup_service(
        algorithms: &[&str],
        max_size: usize,
    ) -> DecompressionService<MockEchoService> {
        DecompressionLayer::new(algorithms, max_size).layer(MockEchoService)
    }

    async fn get_response_body(response: HttpResponse) -> Vec<u8> {
        response.into_body().collect().await.unwrap().to_bytes().to_vec()
    }

    fn compress_gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn compress_deflate(data: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn compress_brotli(data: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let mut encoder = brotli::CompressorWriter::new(&mut compressed, 4096, 11, 22);
            encoder.write_all(data).unwrap();
            encoder.flush().unwrap();
        }
        compressed
    }

    fn compress_zstd(data: &[u8]) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let mut encoder = zstd::Encoder::new(&mut compressed, 3).unwrap();
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap();
        }
        compressed
    }

    fn build_compressed_request(encoding: &str, body: Vec<u8>) -> HttpRequest {
        HttpRequest::builder()
            .header(CONTENT_ENCODING, encoding)
            .body(HttpBody::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn configured_algorithms_are_decompressed_without_content_length() {
        let cases: [(&str, Compressor); 4] = [
            ("zstd", compress_zstd),
            ("gzip", compress_gzip),
            ("deflate", compress_deflate),
            ("br", compress_brotli),
        ];

        for (algorithm, compress) in cases {
            let mut service = setup_service(&[algorithm], DEFAULT_MAX_SIZE);
            let request = build_compressed_request(algorithm, compress(TEST_DATA.as_bytes()));
            let response = service.call(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::OK, "algorithm: {algorithm}");
            assert_eq!(get_response_body(response).await, TEST_DATA.as_bytes());
        }
    }

    #[tokio::test]
    async fn oversized_decompressed_body_is_rejected() {
        const MAX_SIZE: usize = 1024;
        let mut service = setup_service(&["gzip"], MAX_SIZE);
        let body = vec![0; MAX_SIZE + 1];
        let response =
            service.call(build_compressed_request("gzip", compress_gzip(&body))).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn oversized_compressed_body_without_content_length_is_rejected() {
        let compressed = compress_gzip(TEST_DATA.as_bytes());
        let max_size = compressed.len() - 1;
        assert!(TEST_DATA.len() <= max_size);

        let mut service = setup_service(&["gzip"], max_size);
        let response = service.call(build_compressed_request("gzip", compressed)).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn oversized_compressed_content_length_is_rejected() {
        let compressed = compress_gzip(TEST_DATA.as_bytes());
        let max_size = compressed.len();
        let mut request = build_compressed_request("gzip", compressed);
        request.headers_mut().insert(http::header::CONTENT_LENGTH, (max_size + 1).into());

        let mut service = setup_service(&["gzip"], max_size);
        let response = service.call(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn malformed_compressed_body_is_rejected() {
        let mut service = setup_service(&["gzip"], DEFAULT_MAX_SIZE);
        let response = service
            .call(build_compressed_request("gzip", b"not a gzip stream".to_vec()))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn disabled_algorithm_is_rejected() {
        let mut service = setup_service(&["gzip"], DEFAULT_MAX_SIZE);
        let mut request = build_compressed_request("zstd", compress_zstd(TEST_DATA.as_bytes()));
        request.headers_mut().insert(http::header::CONTENT_LENGTH, (DEFAULT_MAX_SIZE + 1).into());

        let response = service.call(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn mixed_case_content_codings_are_accepted() {
        let cases: [(&str, Compressor); 3] =
            [("GZip", compress_gzip), ("ZSTD", compress_zstd), ("Br", compress_brotli)];

        for (encoding, compress) in cases {
            let mut service =
                setup_service(&[encoding.to_ascii_lowercase().as_str()], DEFAULT_MAX_SIZE);
            let request = build_compressed_request(encoding, compress(TEST_DATA.as_bytes()));
            let response = service.call(request).await.unwrap();

            assert_eq!(response.status(), StatusCode::OK, "encoding: {encoding}");
            assert_eq!(get_response_body(response).await, TEST_DATA.as_bytes());
        }
    }

    #[tokio::test]
    async fn concurrent_compressed_requests_all_complete() {
        let service = setup_service(&["gzip"], DEFAULT_MAX_SIZE);
        let mut tasks = tokio::task::JoinSet::new();

        // Spawn more requests than there are decompression permits to ensure permits are
        // released and waiting requests complete.
        for _ in 0..4 * MAX_CONCURRENT_DECOMPRESSIONS {
            let mut service = service.clone();
            tasks.spawn(async move {
                let request = build_compressed_request("gzip", compress_gzip(TEST_DATA.as_bytes()));
                service.call(request).await.unwrap()
            });
        }

        while let Some(response) = tasks.join_next().await {
            assert_eq!(response.unwrap().status(), StatusCode::OK);
        }
    }

    #[tokio::test]
    async fn identity_and_unencoded_bodies_are_not_eagerly_collected() {
        for encoding in [None, Some("identity"), Some("Identity")] {
            let mut service = setup_service(&["gzip"], DEFAULT_MAX_SIZE);
            let body = Limited::new(HttpBody::from(TEST_DATA), 0);
            let mut request = HttpRequest::builder();
            if let Some(encoding) = encoding {
                request = request.header(CONTENT_ENCODING, encoding);
            }

            let response = service.call(request.body(HttpBody::new(body)).unwrap()).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
    }
}
