//! Virtual host filtering middleware for HTTP and WebSocket servers.
//!
//! This module provides a middleware layer that validates the `Host` header in incoming requests
//! against a list of allowed virtual hostnames. It supports wildcard matching with '*' character.

use http::{Response, StatusCode};
use jsonrpsee_http_client::{HttpBody, HttpRequest, HttpResponse};
use pin_project::pin_project;
use std::{
    future::Future,
    net::IpAddr,
    pin::Pin,
    str::FromStr,
    task::{Context, Poll},
};
use tower::{Layer, Service};

/// A trait for validating virtual hostnames from the Host header.
pub trait VHostValidator {
    /// Validates the Host header value against allowed virtual hostnames.
    /// Returns `Ok(())` if the host is allowed, or an error response if not.
    #[expect(clippy::result_large_err)]
    fn validate(&self, host: Option<&str>) -> Result<(), HttpResponse>;
}

/// A validator that checks Host headers against a list of allowed virtual hostnames.
///
/// Supports wildcard matching with '*' character. For example:
/// - `"*"` matches all hosts
/// - `"*.example.com"` matches any subdomain of example.com
/// - `"localhost"` matches exactly "localhost"
#[derive(Clone, Debug)]
pub struct AllowedVHosts {
    /// List of allowed virtual hostname patterns
    patterns: Vec<String>,
}

impl AllowedVHosts {
    /// Creates a new validator from a comma-separated string of hostnames.
    ///
    /// # Example
    /// ```
    /// use reth_rpc_layer::AllowedVHosts;
    ///
    /// let validator = AllowedVHosts::from_str("localhost,*.example.com");
    /// ```
    pub fn from_str(vhosts: &str) -> Self {
        let patterns =
            vhosts.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
        Self { patterns }
    }

    /// Checks if a hostname matches any of the allowed patterns.
    fn matches(&self, host: Option<&str>) -> bool {
        let Some(host) = host else {
            return false;
        };

        // Handle IPv6 addresses which may be in brackets: [::1] or [::1]:8080
        let host_for_ip_check = if host.starts_with('[') {
            // Extract IPv6 address from brackets: [::1] or [::1]:8080
            if let Some(end_bracket) = host.find(']') {
                &host[1..end_bracket]
            } else {
                // Malformed bracket, try parsing the whole string
                host
            }
        } else {
            // For IPv4 or IPv6 without brackets, try parsing directly first
            // If it's an IPv6 address like "::1", it will parse successfully
            // If it's an IPv4 address like "127.0.0.1", it will parse successfully
            // If it's a domain with port like "localhost:8545", parsing will fail
            if IpAddr::from_str(host).is_ok() {
                return true;
            }
            // If parsing failed, it might be a domain with port, remove port
            host.split(':').next().unwrap_or(host)
        };

        // IP addresses (IPv4 or IPv6) are always allowed, regardless of vhosts configuration
        if IpAddr::from_str(host_for_ip_check).is_ok() {
            return true;
        }

        // For domain names, check against patterns
        let host = host.to_lowercase();
        // Remove port if present (for domains, port is after the last colon)
        // But be careful: IPv6 addresses in brackets already handled above
        let host_without_port = if host.starts_with('[') {
            // IPv6 with brackets, extract the address part
            if let Some(end_bracket) = host.find(']') {
                &host[1..end_bracket]
            } else {
                &host
            }
        } else {
            // For domains, split by ':' to remove port
            host.split(':').next().unwrap_or(&host)
        };

        for pattern in &self.patterns {
            if Self::pattern_matches(pattern, host_without_port) {
                return true;
            }
        }

        false
    }

    /// Checks if a hostname matches a pattern, supporting '*' wildcard.
    fn pattern_matches(pattern: &str, host: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        // Handle wildcard patterns like "*.example.com"
        if pattern.starts_with("*.") {
            let suffix = &pattern[2..];
            // For "*.example.com", match "sub.example.com" but not "example.com"
            // The host must end with the suffix AND have at least one dot before it
            if host == suffix {
                return false;
            }
            return host.ends_with(suffix) && host.len() > suffix.len() + 1;
        }

        // Exact match
        pattern == host
    }
}

impl VHostValidator for AllowedVHosts {
    fn validate(&self, host: Option<&str>) -> Result<(), HttpResponse> {
        if self.matches(host) {
            Ok(())
        } else {
            let host_str = host.unwrap_or("(missing)");
            let response = Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(HttpBody::new(format!("Virtual host '{}' not allowed", host_str)))
                .expect("This should never happen");
            Err(response)
        }
    }
}

/// HTTP middleware layer that validates virtual hostnames from the Host header.
///
/// This layer checks incoming requests' Host header against a list of allowed virtual hostnames.
/// Requests with disallowed hosts are rejected with a 403 Forbidden response.
///
/// # Example
/// ```rust
/// use reth_rpc_layer::{AllowedVHosts, VHostLayer};
/// use tower::ServiceBuilder;
///
/// let validator = AllowedVHosts::from_str("localhost,*.example.com");
/// let layer = VHostLayer::new(validator);
/// let middleware = ServiceBuilder::new().layer(layer);
/// ```
#[expect(missing_debug_implementations)]
pub struct VHostLayer<V> {
    validator: V,
}

impl<V> VHostLayer<V> {
    /// Creates a new [`VHostLayer`] with the given validator.
    pub const fn new(validator: V) -> Self {
        Self { validator }
    }
}

impl<S, V> Layer<S> for VHostLayer<V>
where
    V: Clone,
{
    type Service = VHostService<S, V>;

    fn layer(&self, inner: S) -> Self::Service {
        VHostService { validator: self.validator.clone(), inner }
    }
}

/// Service implementation that validates Host headers before forwarding requests.
#[derive(Clone, Debug)]
pub struct VHostService<S, V> {
    /// Validates the Host header
    validator: V,
    /// Inner service that handles validated requests
    inner: S,
}

impl<S, V> Service<HttpRequest> for VHostService<S, V>
where
    S: Service<HttpRequest, Response = HttpResponse>,
    V: VHostValidator,
    Self: Clone,
{
    type Response = HttpResponse;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: HttpRequest) -> Self::Future {
        // Extract Host header
        let host = req.headers().get(http::header::HOST).and_then(|h| h.to_str().ok());

        match self.validator.validate(host) {
            Ok(_) => ResponseFuture::future(self.inner.call(req)),
            Err(res) => ResponseFuture::invalid_vhost(res),
        }
    }
}

/// A future representing the response of an RPC request
#[pin_project]
#[expect(missing_debug_implementations)]
pub struct ResponseFuture<F> {
    /// The kind of response future, error or pending
    #[pin]
    kind: Kind<F>,
}

impl<F> ResponseFuture<F> {
    const fn future(future: F) -> Self {
        Self { kind: Kind::Future { future } }
    }

    const fn invalid_vhost(err_res: HttpResponse) -> Self {
        Self { kind: Kind::Error { response: Some(err_res) } }
    }
}

#[pin_project(project = KindProj)]
enum Kind<F> {
    Future {
        #[pin]
        future: F,
    },
    Error {
        response: Option<HttpResponse>,
    },
}

impl<F, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<HttpResponse, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().kind.project() {
            KindProj::Future { future } => future.poll(cx),
            KindProj::Error { response } => {
                let response = response.take().unwrap();
                Poll::Ready(Ok(response))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_vhosts_from_str() {
        let validator = AllowedVHosts::from_str("localhost,*.example.com");
        assert!(validator.matches(Some("localhost")));
        assert!(validator.matches(Some("test.example.com")));
        assert!(validator.matches(Some("sub.test.example.com")));
        assert!(!validator.matches(Some("example.com")));
        assert!(!validator.matches(Some("other.com")));
    }

    #[test]
    fn test_wildcard_all() {
        let validator = AllowedVHosts::from_str("*");
        assert!(validator.matches(Some("localhost")));
        assert!(validator.matches(Some("example.com")));
        assert!(validator.matches(Some("any.host.com")));
    }

    #[test]
    fn test_wildcard_prefix() {
        let validator = AllowedVHosts::from_str("*.example.com");
        assert!(validator.matches(Some("test.example.com")));
        assert!(validator.matches(Some("sub.test.example.com")));
        assert!(!validator.matches(Some("example.com")));
        assert!(!validator.matches(Some("other.com")));
    }

    #[test]
    fn test_exact_match() {
        let validator = AllowedVHosts::from_str("localhost,example.com");
        assert!(validator.matches(Some("localhost")));
        assert!(validator.matches(Some("example.com")));
        assert!(!validator.matches(Some("test.example.com")));
    }

    #[test]
    fn test_host_with_port() {
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("localhost:8545")));
        assert!(validator.matches(Some("localhost:8080")));
    }

    #[test]
    fn test_case_insensitive() {
        let validator = AllowedVHosts::from_str("LocalHost,EXAMPLE.COM");
        assert!(validator.matches(Some("localhost")));
        assert!(validator.matches(Some("LOCALHOST")));
        assert!(validator.matches(Some("example.com")));
        assert!(validator.matches(Some("EXAMPLE.COM")));
    }

    #[test]
    fn test_missing_host() {
        let validator = AllowedVHosts::from_str("localhost");
        assert!(!validator.matches(None));
    }

    #[test]
    fn test_empty_patterns() {
        let validator = AllowedVHosts::from_str("");
        assert!(!validator.matches(Some("localhost")));
    }

    #[test]
    fn test_whitespace_handling() {
        let validator = AllowedVHosts::from_str("localhost , example.com , *.test.com");
        assert!(validator.matches(Some("localhost")));
        assert!(validator.matches(Some("example.com")));
        assert!(validator.matches(Some("sub.test.com")));
    }

    #[test]
    fn test_ipv4_address_allowed() {
        // IP addresses should always be allowed, regardless of vhosts configuration
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("127.0.0.1")));
        assert!(validator.matches(Some("192.168.1.1")));
        assert!(validator.matches(Some("0.0.0.0")));
        assert!(validator.matches(Some("255.255.255.255")));
    }

    #[test]
    fn test_ipv4_address_with_port_allowed() {
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("127.0.0.1:8545")));
        assert!(validator.matches(Some("192.168.1.1:8080")));
        assert!(validator.matches(Some("0.0.0.0:3000")));
    }

    #[test]
    fn test_ipv6_address_allowed() {
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("::1")));
        assert!(validator.matches(Some("2001:0db8:85a3:0000:0000:8a2e:0370:7334")));
        assert!(validator.matches(Some("fe80::1")));
    }

    #[test]
    fn test_ipv6_address_with_port_allowed() {
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("[::1]:8545")));
        assert!(validator.matches(Some("[2001:0db8:85a3::8a2e:0370:7334]:8080")));
    }

    #[test]
    fn test_ip_allowed_even_with_strict_vhosts() {
        // Even with strict vhosts (only "localhost"), IPs should be allowed
        let validator = AllowedVHosts::from_str("localhost");
        assert!(validator.matches(Some("127.0.0.1")));
        assert!(validator.matches(Some("::1")));
        assert!(validator.matches(Some("192.168.1.1:8545")));
    }

    #[tokio::test]
    async fn test_vhost_layer_integration() {
        // We group all tests into one to avoid individual #[tokio::test]
        // to concurrently spawn a server on the same port.
        valid_host().await;
        invalid_host().await;
        missing_host().await;
        wildcard_all().await;
        wildcard_prefix().await;
    }

    async fn valid_host() {
        let (status, _) = send_request(Some("localhost")).await;
        assert_eq!(status, 200);
    }

    async fn invalid_host() {
        let (status, body) = send_request(Some("invalid.com")).await;
        assert_eq!(status, 403);
        assert!(body.contains("not allowed"));
    }

    async fn missing_host() {
        // Note: reqwest automatically adds Host header from URL (127.0.0.1:8551),
        // which is an IP address and should be allowed.
        // To test truly missing Host header, we'd need a lower-level HTTP client.
        // For now, we test that IP addresses are allowed even when not in vhosts list.
        let (status, _) = send_request(None).await;
        assert_eq!(status, 200); // IP address should be allowed
    }

    async fn wildcard_all() {
        let server = spawn_server_with_vhosts("*").await;
        let client =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(1)).build().unwrap();

        let body = r#"{"jsonrpc": "2.0", "method": "test", "params": [], "id": 1}"#;
        let response = client
            .post("http://127.0.0.1:8552")
            .header(reqwest::header::HOST, "any.host.com")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        let status = response.status();

        server.stop().unwrap();
        server.stopped().await;

        assert_eq!(status, 200);
    }

    async fn wildcard_prefix() {
        let server = spawn_server_with_vhosts("*.example.com").await;
        let client =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(1)).build().unwrap();

        let body = r#"{"jsonrpc": "2.0", "method": "test", "params": [], "id": 1}"#;
        let response = client
            .post("http://127.0.0.1:8553")
            .header(reqwest::header::HOST, "test.example.com")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .unwrap();
        let status = response.status();

        server.stop().unwrap();
        server.stopped().await;

        assert_eq!(status, 200);
    }

    async fn send_request(host: Option<&str>) -> (u16, String) {
        let server = spawn_server_with_vhosts("localhost").await;
        let client =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(1)).build().unwrap();

        let body = r#"{"jsonrpc": "2.0", "method": "test", "params": [], "id": 1}"#;
        let mut request = client
            .post("http://127.0.0.1:8554") // Changed from 8551 to avoid conflict with auth_layer tests
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);

        if let Some(host) = host {
            request = request.header(reqwest::header::HOST, host);
        }

        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();

        server.stop().unwrap();
        server.stopped().await;

        (status, body)
    }

    /// Spawn a new RPC server equipped with a `VHostLayer` middleware.
    async fn spawn_server_with_vhosts(vhosts: &str) -> jsonrpsee::server::ServerHandle {
        use jsonrpsee::{
            server::{RandomStringIdProvider, ServerBuilder, ServerConfig},
            RpcModule,
        };
        use std::net::SocketAddr;

        let validator = AllowedVHosts::from_str(vhosts);
        let layer = VHostLayer::new(validator);
        let middleware = tower::ServiceBuilder::default().layer(layer);

        // Use different ports for different test cases
        // Note: 8551 is used by auth_layer tests, so we use different ports
        let port = match vhosts {
            "*" => 8552,
            "*.example.com" => 8553,
            _ => 8554, // Changed from 8551 to avoid conflict with auth_layer tests
        };
        let addr = format!("127.0.0.1:{}", port);

        // Create a layered server
        let server = ServerBuilder::default()
            .set_config(
                ServerConfig::builder().set_id_provider(RandomStringIdProvider::new(16)).build(),
            )
            .set_http_middleware(middleware)
            .build(addr.parse::<SocketAddr>().unwrap())
            .await
            .unwrap();

        // Create a mock rpc module
        let mut module = RpcModule::new(());
        module.register_method("test", |_, _, _| "test response").unwrap();

        server.start(module)
    }
}
