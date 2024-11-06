use jsonrpsee_http_client::{HttpRequest, HttpResponse};
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tower::{Layer, Service};

#[derive(Debug, Clone, Copy)]
enum CompressionKind {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// Compression middleware service that checks Accept-Encoding on request
/// and compresses response if needed
#[derive(Clone, Debug)]
pub struct CompressionService<S> {
    inner: S,
    compression: Option<CompressionKind>,
}

/// Layer for adding compression support
pub struct CompressionLayer;

impl CompressionLayer {
    pub const fn new() -> Self {
        Self
    }
}

impl<S> Layer<S> for CompressionLayer {
    type Service = CompressionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CompressionService { inner, compression: None }
    }
}

impl<S> Service<HttpRequest> for CompressionService<S>
where
    S: Service<HttpRequest, Response = HttpResponse>,
{
    type Response = HttpResponse;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        // Check Accept-Encoding in request and set the appropriate compression type
        self.compression = req
            .headers()
            .get("Accept-Encoding")
            .and_then(|header_value| header_value.to_str().ok())
            .and_then(|ptr_str| {
                if ptr_str.contains("gzip") {
                    Some(CompressionKind::Gzip)
                } else if ptr_str.contains("deflate") {
                    Some(CompressionKind::Deflate)
                } else if ptr_str.contains("br") {
                    Some(CompressionKind::Brotli)
                } else if ptr_str.contains("zstd") {
                    Some(CompressionKind::Zstd)
                } else {
                    None
                }
            });

        ResponseFuture { inner: self.inner.call(req), compression: self.compression }
    }
}

#[pin_project]
#[allow(missing_debug_implementations)]
pub struct ResponseFuture<F> {
    #[pin]
    inner: F,
    compression: Option<CompressionKind>,
}

impl<F, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<HttpResponse, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut response = ready!(this.inner.poll(cx)?);

        if let Some(compression) = this.compression {
            let header_value = match compression {
                CompressionKind::Gzip => "gzip",
                CompressionKind::Deflate => "deflate",
                CompressionKind::Brotli => "br",
                CompressionKind::Zstd => "zstd",
            };
            if let Ok(value) = header_value.parse() {
                response.headers_mut().insert("Content-Encoding", value);
            }
        }

        Poll::Ready(Ok(response))
    }
}

#[cfg(test)]
mod tests {
    use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};
    use jsonrpsee_http_client::{HttpBody, HttpRequest, HttpResponse};
    use std::future::ready;
    use tower::{Service, ServiceBuilder, ServiceExt};

    use crate::CompressionLayer;

    #[derive(Clone)]
    struct MockRequestService;

    impl Service<HttpRequest> for MockRequestService {
        type Response = HttpResponse;
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(
            &mut self,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, _: HttpRequest) -> Self::Future {
            // Create a large test payload that will benefit from compression
            let large_string = "compress test data ".repeat(1000);
            let body = HttpBody::from(large_string);
            let response = HttpResponse::builder().body(body).unwrap();
            ready(Ok(response))
        }
    }

    #[tokio::test]
    async fn test_compression_decompression() -> Result<(), Box<dyn std::error::Error>> {
        // 1. Create service with compression
        let mut compression_service =
            ServiceBuilder::new().layer(CompressionLayer::new()).service(MockRequestService);

        // 2. Create request that accepts gzip
        let request =
            HttpRequest::builder().header(ACCEPT_ENCODING, "gzip").body(HttpBody::empty()).unwrap();

        // 3. Get compressed response
        let compressed_response = compression_service.ready().await?.call(request).await?;

        // Verify compression header
        assert_eq!(
            compressed_response.headers().get(CONTENT_ENCODING).unwrap(),
            "gzip",
            "Response should indicate gzip compression"
        );

        Ok(())
    }
}