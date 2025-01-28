use jsonrpsee_http_client::{HttpBody, HttpRequest, HttpResponse};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};
use tower_http::compression::{Compression, CompressionLayer as TowerCompressionLayer};

/// This layer is a wrapper around [`tower_http::compression::CompressionLayer`] that integrates
/// with jsonrpsee's HTTP types. It automatically compresses responses based on the client's
/// Accept-Encoding header.
#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct CompressionLayer {
    inner_layer: TowerCompressionLayer,
}

impl CompressionLayer {
    /// Creates a new compression layer with zstd, gzip, brotli and deflate enabled.
    pub fn new() -> Self {
        Self {
            inner_layer: TowerCompressionLayer::new().gzip(true).br(true).deflate(true).zstd(true),
        }
    }
}

impl Default for CompressionLayer {
    /// Creates a new compression layer with default settings.
    /// See [`CompressionLayer::new`] for details.
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for CompressionLayer {
    type Service = CompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CompressionService { compression: self.inner_layer.layer(inner) }
    }
}

/// Service that performs response compression.
///
/// Created by [`CompressionLayer`].
#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct CompressionService<S> {
    compression: Compression<S>,
}

impl<S> Service<HttpRequest> for CompressionService<S>
where
    S: Service<HttpRequest, Response = HttpResponse>,
    S::Future: Send + 'static,
{
    type Response = HttpResponse;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.compression.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        let fut = self.compression.call(req);

        Box::pin(async move {
            let resp = fut.await?;
            let (parts, compressed_body) = resp.into_parts();
            let http_body = HttpBody::new(compressed_body);

            Ok(Self::Response::from_parts(parts, http_body))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};
    use http_body_util::BodyExt;
    use jsonrpsee_http_client::{HttpRequest, HttpResponse};
    use std::{convert::Infallible, future::ready};

    const TEST_DATA: &str = "compress test data ";
    const REPEAT_COUNT: usize = 1000;

    #[derive(Clone)]
    struct MockRequestService;

    impl Service<HttpRequest> for MockRequestService {
        type Response = HttpResponse;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(
            &mut self,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: HttpRequest) -> Self::Future {
            let body = HttpBody::from(TEST_DATA.repeat(REPEAT_COUNT));
            let response = HttpResponse::builder().body(body).unwrap();
            ready(Ok(response))
        }
    }

    fn setup_compression_service(
    ) -> impl Service<HttpRequest, Response = HttpResponse, Error = Infallible> {
        CompressionLayer::new().layer(MockRequestService)
    }

    async fn get_response_size(response: HttpResponse) -> usize {
        // Get the total size of the response body
        response.into_body().collect().await.unwrap().to_bytes().len()
    }

    #[tokio::test]
    async fn test_gzip_compression() {
        let mut service = setup_compression_service();
        let request =
            HttpRequest::builder().header(ACCEPT_ENCODING, "gzip").body(HttpBody::empty()).unwrap();

        let uncompressed_len = TEST_DATA.repeat(REPEAT_COUNT).len();

        // Make the request
        let response = service.call(request).await.unwrap();

        // Verify the response has gzip content-encoding
        assert_eq!(
            response.headers().get(CONTENT_ENCODING).unwrap(),
            "gzip",
            "Response should be gzip encoded"
        );

        // Verify the response body is actually compressed (should be smaller than original)
        let compressed_size = get_response_size(response).await;
        assert!(
            compressed_size < uncompressed_len,
            "Compressed size ({compressed_size}) should be smaller than original size ({uncompressed_len})"
        );
    }

    #[tokio::test]
    async fn test_no_compression_when_not_requested() {
        // Create a service with compression
        let mut service = setup_compression_service();
        let request = HttpRequest::builder().body(HttpBody::empty()).unwrap();

        let response = service.call(request).await.unwrap();
        assert!(
            response.headers().get(CONTENT_ENCODING).is_none(),
            "Response should not be compressed when not requested"
        );

        let uncompressed_len = TEST_DATA.repeat(REPEAT_COUNT).len();

        // Verify the response body matches the original size
        let response_size = get_response_size(response).await;
        assert!(
            response_size == uncompressed_len,
            "Response size ({response_size}) should equal original size ({uncompressed_len})"
        );
    }
}
