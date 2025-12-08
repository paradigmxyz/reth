use alloy_primitives::bytes::BytesMut;
use jsonrpsee_http_client::{HttpBody, HttpRequest, HttpResponse};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};
use tower_http::decompression::{
    RequestDecompression, RequestDecompressionLayer as TowerDecompressionLayer,
};

/// Standard buffer size for decompressed payloads.
const BUF_SIZE: usize = 128 * 1024; // 128kb

/// This layer is a wrapper around [`tower_http::decompression::RequestDecompressionLayer`] that
/// integrates with jsonrpsee's HTTP types.
#[expect(missing_debug_implementations)]
#[derive(Clone)]
pub struct DecompressionLayer {
    inner_layer: TowerDecompressionLayer,
    /// Maximum size in bytes for the decompressed body
    max_body_size: usize,
}

impl DecompressionLayer {
    /// Creates a new decompression layer from a list of algorithm names.
    /// Supported: zstd, gzip, deflate, br
    pub fn new(algos: &[impl AsRef<str>], max_body_size: usize) -> Self {
        // Start with all algorithms explicitly disabled
        let mut layer =
            TowerDecompressionLayer::new()
            .no_zstd()
            .no_gzip()
            .no_deflate()
            .no_br();

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

        Self { inner_layer: layer, max_body_size }
    }
}

impl<S> Layer<S> for DecompressionLayer {
    type Service = DecompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DecompressionService {
            decompression: self
                .inner_layer
                .layer(InnerService { inner, max_body_size: self.max_body_size }),
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
}

/// Inner service wrapper to handle type conversion between jsonrpsee and `tower_http`
/// with body size limiting.
#[derive(Clone)]
struct InnerService<S> {
    inner: S,
    max_body_size: usize,
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

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(
        &mut self,
        req: http::Request<tower_http::decompression::DecompressionBody<HttpBody>>,
    ) -> Self::Future {
        let mut inner = self.inner.clone();
        let max_body_size = self.max_body_size;
        Box::pin(async move {
            let (parts, body) = req.into_parts();

            use http_body_util::BodyExt;
            let initial_capacity = std::cmp::min(BUF_SIZE, max_body_size);
            let mut collected_bytes = BytesMut::with_capacity(initial_capacity);
            let mut body = body;

            while let Some(chunk_result) = body.frame().await {
                match chunk_result {
                    Ok(frame) => {
                        if let Some(chunk) = frame.data_ref() {
                            if collected_bytes.len() + chunk.len() > max_body_size {
                                collected_bytes.clear();
                                return Ok(err_response(413, "Payload Too Large"));
                            }
                            collected_bytes.extend_from_slice(chunk);
                        }
                    }
                    Err(e) => {
                        collected_bytes.clear();
                        let error_msg = format!("Failed to decompress request body: {}", e);
                        return Ok(err_response(400, &error_msg));
                    }
                }
            }

            let http_body = HttpBody::from(collected_bytes.freeze().to_vec());
            let http_request = HttpRequest::from_parts(parts, http_body);

            // Call the inner service
            let http_response = inner.call(http_request).await?;
            Ok(http_response)
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

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        // Inspect headers before handing the request to the decompression layer.
        let has_encoding: bool = req.headers().contains_key(http::header::CONTENT_ENCODING);
        let has_length = req.headers().contains_key(http::header::CONTENT_LENGTH);

        if has_encoding && !has_length {
            return Box::pin(async { Ok(err_response(411, "Length Required")) });
        }
        let fut = self.decompression.call(req);

        Box::pin(async move {
            let resp = match fut.await {
                Ok(r) => r,
                Err(_) => {
                    return Ok(err_response(500, "Internal Server Error"));
                }
            };
            let (parts, unsync_body) = resp.into_parts();
            let http_body = HttpBody::new(unsync_body);
            Ok(HttpResponse::from_parts(parts, http_body))
        })
    }
}

#[inline]
fn err_response(status: u16, msg: &str) -> HttpResponse {
    http::Response::builder()
        .status(status)
        .header("Content-Type", "text/plain")
        .body(HttpBody::from(msg.as_bytes().to_vec()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
    use http_body_util::BodyExt;
    use jsonrpsee_http_client::{HttpRequest, HttpResponse};
    use std::{convert::Infallible, future::ready, io::Write};

    const TEST_DATA: &str = r#"{"method":"test","params":["test data"],"id":1}"#;
    const MB: usize = 1024 * 1024;
    const DEFAULT_MAX_SIZE: usize = 15 * MB;

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
        max_size: usize,
    ) -> impl Service<HttpRequest, Response = HttpResponse, Error = Infallible> + Clone {
        DecompressionLayer::new(&["gzip", "deflate", "br", "zstd"], max_size).layer(MockEchoService)
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

    fn compress_zstd(data: &[u8], with_contentsize: bool) -> Vec<u8> {
        let mut compressed = Vec::new();
        {
            let mut encoder = zstd::Encoder::new(&mut compressed, 3).unwrap();
            encoder.include_contentsize(with_contentsize).unwrap();
            if with_contentsize {
                encoder.set_pledged_src_size(Some(data.len() as u64)).unwrap();
            }
            encoder.write_all(data).unwrap();
            encoder.finish().unwrap();
        }
        compressed
    }

    fn build_compressed_request(encoding: &str, body: Vec<u8>) -> HttpRequest {
        HttpRequest::builder()
            .header(CONTENT_ENCODING, encoding)
            .header(CONTENT_LENGTH, body.len().to_string())
            .body(HttpBody::from(body))
            .unwrap()
    }

    macro_rules! test_decompress {
        ($name:ident, $encoding:expr, $compress_fn:expr) => {
            #[tokio::test]
            async fn $name() {
                let mut service = setup_service(DEFAULT_MAX_SIZE);
                let request =
                    build_compressed_request($encoding, $compress_fn(TEST_DATA.as_bytes()));
                let response = service.call(request).await.unwrap();
                let body = get_response_body(response).await;
                assert_eq!(body, TEST_DATA.as_bytes());
            }
        };
    }

    test_decompress!(test_gzip_decompression, "gzip", compress_gzip);
    test_decompress!(test_deflate_decompression, "deflate", compress_deflate);
    test_decompress!(test_brotli_decompression, "br", compress_brotli);

    #[tokio::test]
    async fn test_zstd_decompression() {
        let mut service = setup_service(DEFAULT_MAX_SIZE);
        let request = build_compressed_request("zstd", compress_zstd(TEST_DATA.as_bytes(), true));
        let response = service.call(request).await.unwrap();
        let body = get_response_body(response).await;
        assert_eq!(body, TEST_DATA.as_bytes());
    }

    #[tokio::test]
    async fn test_no_decompression_when_not_needed() {
        let mut service = setup_service(DEFAULT_MAX_SIZE);
        let response = service
            .call(HttpRequest::builder().body(HttpBody::from(TEST_DATA)).unwrap())
            .await
            .unwrap();
        assert_eq!(get_response_body(response).await, TEST_DATA.as_bytes());
    }

    #[tokio::test]
    async fn test_identity_encoding_passthrough() {
        let mut service = setup_service(DEFAULT_MAX_SIZE);
        let request = HttpRequest::builder()
            .header(CONTENT_ENCODING, "identity")
            .header(CONTENT_LENGTH, TEST_DATA.len().to_string())
            .body(HttpBody::from(TEST_DATA))
            .unwrap();
        let response = service.call(request).await.unwrap();
        assert_eq!(get_response_body(response).await, TEST_DATA.as_bytes());
    }

    #[tokio::test]
    async fn test_normal_payload_within_limit() {
        let mut service = setup_service(MB);
        let response = service
            .call(build_compressed_request("gzip", compress_gzip(TEST_DATA.as_bytes())))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(get_response_body(response).await, TEST_DATA.as_bytes());
    }

    #[tokio::test]
    async fn test_compressed_without_content_length() {
        let mut service = setup_service(DEFAULT_MAX_SIZE);
        let compressed = compress_gzip(TEST_DATA.as_bytes());

        // Build a request with Content-Encoding but without Content-Length.
        let request = HttpRequest::builder()
            .header(CONTENT_ENCODING, "gzip")
            .body(HttpBody::from(compressed))
            .unwrap();

        let response = service.call(request).await.unwrap();
        assert_eq!(response.status(), 411);
    }
}
