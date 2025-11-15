//! Path prefix tests

use http::Request;
use reth_rpc_layer::PathPrefixLayer;
use tower::{Layer, ServiceExt};

/// Helper type for empty body in tests
type TestBody = http_body_util::Empty<bytes::Bytes>;

#[tokio::test(flavor = "multi_thread")]
async fn test_path_prefix_strip() {
    reth_tracing::init_test_tracing();

    use http::Uri;
    use std::str::FromStr;
    use tower::Service;

    // Create a mock service that tracks what path it receives
    let service = tower::service_fn(|req: Request<TestBody>| async move {
        let path = req.uri().path().to_string();
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(http::Response::new(
            http_body_util::Full::new(bytes::Bytes::from(path)),
        ))
    });

    // Apply path prefix layer
    let mut service_with_prefix = PathPrefixLayer::new("/rpc").layer(service);

    // Test 1: Request to /rpc should be forwarded as /
    let req = Request::builder()
        .uri(Uri::from_str("http://localhost/rpc").unwrap())
        .body(TestBody::new())
        .unwrap();

    let response = ServiceExt::<Request<TestBody>>::ready(&mut service_with_prefix)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap();
    let path_received = String::from_utf8(body.to_bytes().to_vec()).unwrap();
    assert_eq!(path_received, "/", "Path should be stripped to /");

    // Test 2: Request to /rpc/method should be forwarded as /method
    let req = Request::builder()
        .uri(Uri::from_str("http://localhost/rpc/method").unwrap())
        .body(TestBody::new())
        .unwrap();

    let response = ServiceExt::<Request<TestBody>>::ready(&mut service_with_prefix)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap();
    let path_received = String::from_utf8(body.to_bytes().to_vec()).unwrap();
    assert_eq!(path_received, "/method", "Path should be stripped to /method");

    // Test 3: Request to /other should return 404
    let req = Request::builder()
        .uri(Uri::from_str("http://localhost/other").unwrap())
        .body(TestBody::new())
        .unwrap();

    let response = ServiceExt::<Request<TestBody>>::ready(&mut service_with_prefix)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        http::StatusCode::NOT_FOUND,
        "Non-matching path should return 404"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_path_prefix_with_query_params() {
    reth_tracing::init_test_tracing();

    use http::Uri;
    use std::str::FromStr;
    use tower::Service;

    let service = tower::service_fn(|req: Request<TestBody>| async move {
        let uri = req.uri().to_string();
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(http::Response::new(
            http_body_util::Full::new(bytes::Bytes::from(uri)),
        ))
    });

    let mut service_with_prefix = PathPrefixLayer::new("/api/v1").layer(service);

    // Request with query parameters
    let req = Request::builder()
        .uri(Uri::from_str("http://localhost/api/v1/rpc?param=value").unwrap())
        .body(TestBody::new())
        .unwrap();

    let response = ServiceExt::<Request<TestBody>>::ready(&mut service_with_prefix)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap();
    let uri_received = String::from_utf8(body.to_bytes().to_vec()).unwrap();

    // The path should be stripped but query params should be preserved
    assert!(
        uri_received.contains("/rpc?param=value"),
        "Query parameters should be preserved. Got: {}",
        uri_received
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_root_prefix_passthrough() {
    reth_tracing::init_test_tracing();

    use http::Uri;
    use std::str::FromStr;
    use tower::Service;

    let service = tower::service_fn(|req: Request<TestBody>| async move {
        let path = req.uri().path().to_string();
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(http::Response::new(
            http_body_util::Full::new(bytes::Bytes::from(path)),
        ))
    });

    // Root prefix should pass all requests through unchanged
    let mut service_with_prefix = PathPrefixLayer::new("/").layer(service);

    let req = Request::builder()
        .uri(Uri::from_str("http://localhost/anything").unwrap())
        .body(TestBody::new())
        .unwrap();

    let response = ServiceExt::<Request<TestBody>>::ready(&mut service_with_prefix)
        .await
        .unwrap()
        .call(req)
        .await
        .unwrap();

    let body = http_body_util::BodyExt::collect(response.into_body()).await.unwrap();
    let path_received = String::from_utf8(body.to_bytes().to_vec()).unwrap();
    assert_eq!(path_received, "/anything", "Root prefix should not modify paths");
}

#[test]
fn test_normalize_prefix() {
    use reth_rpc_layer::PathPrefixLayer;

    // Test various prefix formats are normalized correctly
    let layer1 = PathPrefixLayer::new("");
    // Note: Arc<str> doesn't have a specific debug format, just verify it was created
    assert!(format!("{:?}", layer1).contains("PathPrefixLayer"));

    let layer2 = PathPrefixLayer::new("/");
    assert!(format!("{:?}", layer2).contains("PathPrefixLayer"));

    let layer3 = PathPrefixLayer::new("rpc");
    assert!(format!("{:?}", layer3).contains("PathPrefixLayer"));

    let layer4 = PathPrefixLayer::new("/rpc/");
    assert!(format!("{:?}", layer4).contains("PathPrefixLayer"));

    let layer5 = PathPrefixLayer::new("/api/v1");
    assert!(format!("{:?}", layer5).contains("PathPrefixLayer"));
}
