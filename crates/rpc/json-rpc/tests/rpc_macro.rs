//! Tests for the `rpc` attribute macro.

#![cfg(all(feature = "macros", feature = "client"))]

use bytes::Bytes;
use futures::{channel::mpsc, StreamExt};
use reth_json_rpc::{
    client::{BoxError, Client, ClientBuilder, ClientT, Error, SubscriptionClientT},
    rpc, serve_connection, stop_channel, PendingSubscriptionSink, RpcModule, RpcResult,
    RpcServiceBuilder, ServerConfig, SubscriptionResult, INVALID_PARAMS_CODE,
};
use std::time::Duration;
use tokio::time::timeout;

#[rpc(server, client, namespace = "test")]
trait Test<T> {
    /// Adds two numbers.
    #[method(name = "add", aliases = ["test_plus"])]
    fn add(&self, a: u64, b: Option<u64>) -> RpcResult<u64>;

    #[method(name = "echo")]
    async fn echo(&self, value: T) -> RpcResult<T>;

    #[method(name = "count")]
    async fn count(&self) -> RpcResult<usize>;

    #[subscription(name = "subscribe" => "subscription", unsubscribe = "unsubscribe", item = u64)]
    async fn subscribe(&self, n: u64) -> SubscriptionResult;

    #[subscription(name = "subscribeSync", item = u64)]
    fn subscribe_sync(&self) -> SubscriptionResult;
}

struct TestImpl;

impl TestServer<String> for TestImpl {
    fn add(&self, a: u64, b: Option<u64>) -> RpcResult<u64> {
        Ok(a + b.unwrap_or_default())
    }

    async fn echo(&self, value: String) -> RpcResult<String> {
        Ok(value)
    }

    async fn count(&self) -> RpcResult<usize> {
        Ok(3)
    }

    async fn subscribe(&self, pending: PendingSubscriptionSink, n: u64) -> SubscriptionResult {
        let sink = pending.accept().await?;
        for i in 0..n {
            sink.send(&i).await?;
        }
        sink.closed().await;
        Ok(())
    }

    fn subscribe_sync(&self, pending: PendingSubscriptionSink) -> SubscriptionResult {
        tokio::spawn(async move {
            let sink = pending.accept().await.unwrap();
            sink.send(&7u64).await.unwrap();
            sink.closed().await;
        });
        Ok(())
    }
}

/// Serves a connection over in-memory channels.
fn serve(
    module: RpcModule,
    config: ServerConfig,
) -> (mpsc::UnboundedSender<String>, mpsc::UnboundedReceiver<String>) {
    let (client_tx, server_rx) = mpsc::unbounded::<String>();
    let (server_tx, client_rx) = mpsc::unbounded::<String>();
    let (stop, handle) = stop_channel();
    tokio::spawn(async move {
        let reader = server_rx.map(Bytes::from);
        serve_connection(reader, server_tx, module, &RpcServiceBuilder::new(), &config, stop).await;
        drop(handle);
    });
    (client_tx, client_rx)
}

/// Connects a client to a server over in-memory channels.
fn connect(module: RpcModule) -> Client {
    let (client_tx, client_rx) = serve(module, ServerConfig::default());
    ClientBuilder::default().build(client_rx.map(Ok::<_, BoxError>), client_tx)
}

#[test]
fn method_names() {
    let module = TestImpl.into_rpc();
    let mut names = module.method_names().collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "test_add",
            "test_count",
            "test_echo",
            "test_plus",
            "test_subscribe",
            "test_subscribeSync",
            "test_unsubscribe",
            "test_unsubscribeSync",
        ]
    );
}

#[tokio::test]
async fn server_and_client() {
    let client = connect(TestImpl.into_rpc());

    assert_eq!(TestClient::<String>::add(&client, 1, Some(2)).await.unwrap(), 3);
    assert_eq!(TestClient::<String>::add(&client, 1, None).await.unwrap(), 1);
    assert_eq!(client.echo("hi".to_owned()).await.unwrap(), "hi");
    assert_eq!(TestClient::<String>::count(&client).await.unwrap(), 3);

    let res = client.request::<u64, _>("test_plus", reth_json_rpc::rpc_params![2]).await;
    assert_eq!(res.unwrap(), 2);
    let res = client.request::<u64, _>("test_add", reth_json_rpc::rpc_params!["x"]).await;
    assert!(matches!(res, Err(Error::Call(err)) if err.code() == INVALID_PARAMS_CODE));

    let sub = TestClient::<String>::subscribe(&client, 3).await.unwrap();
    assert_eq!(sub.take(3).map(Result::unwrap).collect::<Vec<_>>().await, [0, 1, 2]);
    let mut sub = TestClient::<String>::subscribe_sync(&client).await.unwrap();
    assert_eq!(sub.next().await.unwrap().unwrap(), 7);
    sub.unsubscribe().await.unwrap();

    let params = reth_json_rpc::rpc_params!["x"];
    let res = SubscriptionClientT::subscribe::<u64, _>(
        &client,
        "test_subscribe",
        params,
        "test_unsubscribe",
    )
    .await;
    assert!(matches!(res, Err(Error::Call(err)) if err.code() == INVALID_PARAMS_CODE));
}

/// Subscriptions accepted while all response slots are taken do not wait for each other.
#[tokio::test]
async fn pipelined_subscriptions() {
    let config = ServerConfig::default().set_message_buffer_capacity(2);
    let (tx, mut rx) = serve(TestImpl.into_rpc(), config);
    for id in 0..2 {
        let req =
            format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"test_subscribe","params":[1]}}"#);
        tx.unbounded_send(req).unwrap();
    }
    let batch = r#"[{"jsonrpc":"2.0","id":2,"method":"test_subscribe","params":[1]}]"#;
    tx.unbounded_send(batch.to_owned()).unwrap();

    let mut responses = Vec::new();
    let mut notifications = 0;
    while responses.len() < 3 || notifications < 3 {
        let msg = timeout(Duration::from_secs(10), rx.next()).await.unwrap().unwrap();
        if msg.contains("test_subscription") {
            // Each notification follows the response with its subscription id.
            let value = serde_json::from_str::<serde_json::Value>(&msg).unwrap();
            let sub_id = &value["params"]["subscription"];
            assert!(responses.iter().any(|res: &serde_json::Value| {
                let res = if res.is_array() { &res[0] } else { res };
                res["result"] == *sub_id
            }));
            notifications += 1;
        } else {
            responses.push(serde_json::from_str(&msg).unwrap());
        }
    }
    assert!(responses.iter().any(|res| res.is_array() && res[0]["id"] == 2));
}
