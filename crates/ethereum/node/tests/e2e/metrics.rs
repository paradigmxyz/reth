//! Node metrics export tests.

use http_body_util::BodyExt;
use jsonrpsee::server::{HttpBody, HttpRequest, HttpResponse, Methods, Server};
use reqwest::{Client, Method};
use reth_chainspec::EthereumHardfork;
use reth_e2e_test_utils::E2ETestSetupExt;
use reth_network::test_utils::unused_tcp_addr;
use reth_node_ethereum::EthereumNode;
use std::{net::SocketAddr, time::Duration};
use tokio::{sync::mpsc, time::timeout};
use tower::{service_fn, ServiceBuilder};

#[tokio::test]
async fn pushes_metrics_without_a_listener() -> eyre::Result<()> {
    assert_metrics_push(None).await
}

#[tokio::test]
async fn supports_pushing_and_scraping_together() -> eyre::Result<()> {
    assert_metrics_push(Some(unused_tcp_addr())).await
}

async fn assert_metrics_push(listen_addr: Option<SocketAddr>) -> eyre::Result<()> {
    let (tx, mut received) = mpsc::channel(1);
    let service = service_fn(move |request: HttpRequest| {
        let tx = tx.clone();
        async move {
            let (parts, body) = request.into_parts();
            tx.send((parts, body.collect().await?.to_bytes())).await?;
            Ok(HttpResponse::new(HttpBody::empty()))
        }
    });
    let gateway = Server::builder()
        .set_http_middleware(ServiceBuilder::new().layer_fn(move |_| service.clone()))
        .build("127.0.0.1:0")
        .await?;
    let addr = gateway.local_addr()?;
    let gateway = gateway.start(Methods::default());
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

    for _ in 0..2 {
        let (parts, body) = timeout(Duration::from_secs(10), received.recv()).await?.unwrap();
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
    drop(received);
    gateway.stop()?;
    node.stop().await?;
    Ok(())
}
