//! Path prefix middleware for RPC servers
//!
//! This module provides middleware that can strip a path prefix from incoming HTTP requests
//! before they are processed by the JSON-RPC handler. This allows serving RPC endpoints
//! under custom paths (e.g., `/rpc` or `/api/v1`) instead of just the root path `/`.

use http::{Request, Response, StatusCode};
use http_body::Body;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// Layer that adds path prefix stripping functionality
#[derive(Debug, Clone)]
pub struct PathPrefixLayer {
    /// The path prefix to strip from requests
    prefix: String,
}

impl PathPrefixLayer {
    /// Creates a new `PathPrefixLayer` with the given prefix
    ///
    /// The prefix will be normalized to ensure it starts with `/` and doesn't end with `/`
    /// unless it's the root path.
    pub fn new(prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        let normalized = Self::normalize_prefix(&prefix);
        Self { prefix: normalized }
    }

    /// Normalizes a path prefix:
    /// - Ensures it starts with `/`
    /// - Removes trailing `/` unless it's the root path
    fn normalize_prefix(prefix: &str) -> String {
        let prefix = prefix.trim();

        // Empty or root path
        if prefix.is_empty() || prefix == "/" {
            return "/".to_string();
        }

        // Ensure prefix starts with /
        let with_leading =
            if prefix.starts_with('/') { prefix.to_string() } else { format!("/{}", prefix) };

        // Remove trailing / if present
        with_leading.trim_end_matches('/').to_string()
    }
}

impl<S> Layer<S> for PathPrefixLayer {
    type Service = PathPrefixService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        PathPrefixService { inner, prefix: self.prefix.clone() }
    }
}

/// Service that strips path prefix from requests
#[derive(Debug, Clone)]
pub struct PathPrefixService<S> {
    inner: S,
    prefix: String,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for PathPrefixService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    ResBody: Body + Default,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = PathPrefixFuture<S::Future, ResBody>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let path = req.uri().path();

        // If prefix is root, pass through unchanged
        if self.prefix == "/" {
            return PathPrefixFuture::Inner(self.inner.call(req));
        }

        // Check if path starts with the prefix
        if path.starts_with(&self.prefix) {
            // Strip the prefix from the path
            let new_path = if path.len() == self.prefix.len() {
                // Path is exactly the prefix, use root
                "/"
            } else if path[self.prefix.len()..].starts_with('/') {
                // Path has content after prefix
                &path[self.prefix.len()..]
            } else {
                // Path doesn't have a `/` after prefix, treat as not matching
                return PathPrefixFuture::NotFound;
            };

            // Build new URI with stripped prefix
            let uri = req.uri();
            let mut parts = uri.clone().into_parts();

            // Reconstruct path and query
            let path_and_query = if let Some(query) = uri.query() {
                format!("{}?{}", new_path, query)
            } else {
                new_path.to_string()
            };

            // Update the path and query
            if let Ok(pq) = path_and_query.parse() {
                parts.path_and_query = Some(pq);
                if let Ok(new_uri) = http::Uri::from_parts(parts) {
                    *req.uri_mut() = new_uri;
                }
            }

            PathPrefixFuture::Inner(self.inner.call(req))
        } else {
            // Path doesn't match prefix, return 404
            PathPrefixFuture::NotFound
        }
    }
}

/// Future for the path prefix service
#[pin_project::pin_project(project = PathPrefixFutureProj)]
#[derive(Debug)]
pub enum PathPrefixFuture<F, B> {
    Inner(#[pin] F),
    NotFound,
    _Phantom(std::marker::PhantomData<B>),
}

impl<F, B, E> Future for PathPrefixFuture<F, B>
where
    F: Future<Output = Result<Response<B>, E>>,
    B: Body + Default,
{
    type Output = Result<Response<B>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project() {
            PathPrefixFutureProj::Inner(f) => f.poll(cx),
            PathPrefixFutureProj::NotFound => {
                // Return 404 Not Found
                let response = Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(B::default())
                    .expect("Failed to build 404 response");
                Poll::Ready(Ok(response))
            }
            PathPrefixFutureProj::_Phantom(_) => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_prefix() {
        assert_eq!(PathPrefixLayer::normalize_prefix(""), "/");
        assert_eq!(PathPrefixLayer::normalize_prefix("/"), "/");
        assert_eq!(PathPrefixLayer::normalize_prefix("rpc"), "/rpc");
        assert_eq!(PathPrefixLayer::normalize_prefix("/rpc"), "/rpc");
        assert_eq!(PathPrefixLayer::normalize_prefix("/rpc/"), "/rpc");
        assert_eq!(PathPrefixLayer::normalize_prefix("rpc/"), "/rpc");
        assert_eq!(PathPrefixLayer::normalize_prefix("/api/v1"), "/api/v1");
        assert_eq!(PathPrefixLayer::normalize_prefix("/api/v1/"), "/api/v1");
    }
}
