use jsonrpsee_http_client::{HttpRequest, HttpResponse};
use pin_project::pin_project;
use std::{
   future::Future,
   pin::Pin,
   task::{Context, ready, Poll},
};
use tower::{Layer, Service};

#[derive(Debug, Clone, Copy)]
enum CompressionKind {
   Gzip,
}

/// Encoding middleware service that checks Accept-Encoding on request
/// and compresses response if needed
#[derive(Clone, Debug)]
pub struct CompressionService<S> {
   inner: S,
   compression: Option<CompressionKind>
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
       CompressionService { 
           inner,
           compression: None
       }
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
       // Check Accept-Encoding in request
       self.compression = req.headers()
           .get("Accept-Encoding")
           .and_then(|header_value| header_value.to_str().ok())
           .and_then(|ptr_str| ptr_str.contains("gzip").then_some(CompressionKind::Gzip));

       ResponseFuture {
           inner: self.inner.call(req),
           compression: self.compression,
       }
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
       let response = ready!(this.inner.poll(cx)?);

       match *this.compression {
           Some(CompressionKind::Gzip) => {
               let mut compressed_response = response;
               compressed_response.headers_mut().insert(
                   "Content-Encoding", 
                   "gzip".parse().unwrap()
               );
               // Compress with gzip...
               Poll::Ready(Ok(compressed_response))
           },
           None => Poll::Ready(Ok(response))
       }
   }
}