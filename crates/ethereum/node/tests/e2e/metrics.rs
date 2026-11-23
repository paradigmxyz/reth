//! Node metrics export tests.

use http_body_util::BodyExt;
use jsonrpsee::server::{serve_with_graceful_shutdown, HttpBody, HttpRequest, HttpResponse};
use reqwest::{header::CONNECTION, Client, Method};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_network::test_utils::unused_tcp_addr;
use reth_node_ethereum::EthereumNode;
use std::{convert::Infallible, future::pending, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use tower::service_fn;

#[tokio::test]
async fn pushes_metrics_without_a_listener() -> eyre::Result<()> {
    assert_metrics_push(None).await
}

#[tokio::test]
async fn supports_pushing_and_scraping_together() -> eyre::Result<()> {
    assert_metrics_push(Some(unused_tcp_addr())).await
}

async fn assert_metrics_push(listen_addr: Option<SocketAddr>) -> eyre::Result<()> {
    let gateway = TcpListener::bind("127.0.0.1:0").await?;
    let addr = gateway.local_addr()?;
    let (node, _) = EthereumNode::test_setup_for(EthereumHardfork::Cancun)
        .with_restartable_nodes()
        .with_node_config_modifier(move |mut config| {
            config.metrics.prometheus = listen_addr;
            config.metrics.push_gateway_url = Some(format!("http://{addr}/metrics/job/reth"));
            config.metrics.push_gateway_interval = Duration::from_millis(100);
            config
        })
        .build_single()
        .await?;

    let (tx, mut received) = mpsc::channel(1);
    let service = service_fn(move |request: HttpRequest<_>| {
        let tx = tx.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = HttpBody::new(body).collect().await.unwrap().to_bytes();
            tx.try_send((parts, body)).unwrap();
            Ok::<_, Infallible>(
                HttpResponse::builder()
                    .header(CONNECTION, "close")
                    .body(HttpBody::empty())
                    .unwrap(),
            )
        }
    });
    for _ in 0..2 {
        timeout(Duration::from_secs(10), async {
            let (stream, _) = gateway.accept().await.unwrap();
            serve_with_graceful_shutdown(stream, service.clone(), pending()).await.unwrap();
        })
        .await?;
        let (parts, body) = received.try_recv()?;
        assert_eq!(parts.method, Method::PUT);
        assert_eq!(parts.uri.path(), "/metrics/job/reth");
        assert!(std::str::from_utf8(&body)?.lines().any(|line| line.starts_with("reth_info{")));
    }
    if let Some(addr) = listen_addr {
        let response = Client::new()
            .get(format!("http://{addr}/metrics"))
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?;
        assert!(response.text().await?.lines().any(|line| line.starts_with("reth_info{")));
    }
    drop(gateway);
    node.stop().await?;
    Ok(())
}
