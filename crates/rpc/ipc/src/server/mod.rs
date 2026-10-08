//! JSON-RPC IPC server implementation

use crate::stream_codec::StreamCodec;
use bytes::Bytes;
use futures::StreamExt;
use interprocess::local_socket::{
    tokio::prelude::LocalSocketListener,
    traits::tokio::{Listener, Stream},
    GenericFilePath, ListenerOptions, ToFsName,
};
use reth_jasonrpeesea::{
    serve_connection, stop_channel, IdProvider, Methods, RpcService, RpcServiceBuilder,
    RpcServiceT, ServerConfig, ServerHandle, StopHandle,
};
use std::{future::ready, io, pin::pin, sync::Arc};
use tokio::{
    io::AsyncWriteExt,
    sync::{oneshot, Semaphore},
};
use tokio_util::codec::{FramedRead, FramedWrite};
use tower::{layer::util::Identity, Layer};
use tracing::{debug, trace};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// IPC server.
#[derive(Debug)]
pub struct IpcServer<RpcMiddleware = Identity> {
    /// The endpoint we listen for incoming transactions
    endpoint: String,
    config: ServerConfig,
    socket_permissions: Option<String>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
}

impl<RpcMiddleware> IpcServer<RpcMiddleware> {
    /// Returns the configured endpoint
    pub fn endpoint(&self) -> String {
        self.endpoint.clone()
    }
}

impl<RpcMiddleware> IpcServer<RpcMiddleware>
where
    RpcMiddleware: Layer<RpcService, Service: RpcServiceT + 'static> + Send + Sync + 'static,
{
    /// Start responding to connections requests.
    ///
    /// This will run on the tokio runtime until the server is stopped or the `ServerHandle` is
    /// dropped.
    ///
    /// ```
    /// use reth_ipc::server::Builder;
    /// use reth_jasonrpeesea::RpcModule;
    /// async fn run_server() -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
    ///     let server = Builder::default().build("/tmp/my-uds".into());
    ///     let mut module = RpcModule::new(());
    ///     module.register_method("say_hello", |_, _| "lo")?;
    ///     let handle = server.start(module).await?;
    ///
    ///     // In this example we don't care about doing shutdown so let's it run forever.
    ///     // You may use the `ServerHandle` to shut it down or manage it yourself.
    ///     let server = tokio::spawn(handle.stopped());
    ///     server.await.unwrap();
    ///     Ok(())
    /// }
    /// ```
    pub async fn start(
        self,
        methods: impl Into<Methods>,
    ) -> Result<ServerHandle, IpcServerStartError> {
        let methods = methods.into();
        let (stop, handle) = stop_channel();
        // Bind on the server runtime so the listener is registered with its reactor.
        let (on_ready, ready) = oneshot::channel();
        let config = self.config.clone();
        config.spawn(async move {
            match self.bind() {
                Ok(listener) => {
                    let _ = on_ready.send(Ok(()));
                    self.run(listener, methods, stop).await;
                }
                Err(err) => {
                    let _ = on_ready.send(Err(err));
                }
            }
        });
        ready.await.expect("server task is running")?;
        Ok(handle)
    }

    fn bind(&self) -> Result<LocalSocketListener, IpcServerStartError> {
        trace!(endpoint = ?self.endpoint, "starting ipc server");

        // ensure the file does not exist
        if cfg!(unix) && std::fs::remove_file(&self.endpoint).is_ok() {
            debug!(endpoint = ?self.endpoint, "removed existing IPC endpoint file");
        }

        let listener = self
            .endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .and_then(|name| ListenerOptions::new().name(name).create_tokio())
            .map_err(|source| IpcServerStartError { endpoint: self.endpoint.clone(), source })?;

        #[cfg(unix)]
        if let Some(perms) = &self.socket_permissions &&
            let Ok(mode) = u32::from_str_radix(&perms.replace("0o", ""), 8)
        {
            let _ = std::fs::set_permissions(&self.endpoint, std::fs::Permissions::from_mode(mode));
        }

        Ok(listener)
    }

    async fn run(self, listener: LocalSocketListener, methods: Methods, stop: StopHandle) {
        let Self { config, rpc_middleware, .. } = self;
        let config = Arc::new(config);
        let rpc_middleware = Arc::new(rpc_middleware);
        let connections = Arc::new(Semaphore::new(config.connection_limit() as usize));
        let mut stopped = pin!(stop.clone().shutdown());

        trace!("accepting ipc connections");
        loop {
            let stream = tokio::select! {
                res = listener.accept() => match res {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::error!(%err, "Failed accepting a new IPC connection");
                        continue
                    }
                },
                () = &mut stopped => break,
            };
            let Ok(permit) = connections.clone().try_acquire_owned() else {
                let (_reader, mut writer) = stream.split();
                let _ = writer.write_all(b"Too many connections. Please try again later.").await;
                continue
            };
            trace!("accepted ipc connection");

            let (reader, writer) = stream.split();
            let reader = FramedRead::new(reader, StreamCodec::stream_incoming())
                .filter_map(|res| ready(res.ok().map(Bytes::from)));
            let writer = FramedWrite::new(writer, StreamCodec::stream_incoming());
            let methods = methods.clone();
            let rpc_middleware = rpc_middleware.clone();
            let config = config.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                serve_connection(reader, writer, methods, &rpc_middleware, &config, stop).await;
                drop(permit);
            });
        }
    }
}

/// Error thrown when server couldn't be started.
#[derive(Debug, thiserror::Error)]
#[error("failed to listen on ipc endpoint `{endpoint}`: {source}")]
pub struct IpcServerStartError {
    endpoint: String,
    #[source]
    source: io::Error,
}

/// Builder to configure and create a JSON-RPC server
#[derive(Debug)]
pub struct Builder<RpcMiddleware = Identity> {
    config: ServerConfig,
    socket_permissions: Option<String>,
    rpc_middleware: RpcServiceBuilder<RpcMiddleware>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            config: ServerConfig::default(),
            socket_permissions: None,
            rpc_middleware: RpcServiceBuilder::new(),
        }
    }
}

impl<RpcMiddleware> Builder<RpcMiddleware> {
    /// Set the maximum size of a request body in bytes. Default is 10 MiB.
    pub fn max_request_body_size(mut self, size: u32) -> Self {
        self.config = self.config.max_request_body_size(size);
        self
    }

    /// Set the maximum size of a response body in bytes. Default is 10 MiB.
    pub fn max_response_body_size(mut self, size: u32) -> Self {
        self.config = self.config.max_response_body_size(size);
        self
    }

    /// Set the maximum number of connections allowed. Default is 100.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.config = self.config.max_connections(max);
        self
    }

    /// Set the maximum number of subscriptions per connection. Default is 1024.
    pub fn max_subscriptions_per_connection(mut self, max: u32) -> Self {
        self.config = self.config.max_subscriptions_per_connection(max);
        self
    }

    /// The server enforces backpressure which means that
    /// `n` messages can be buffered and if the client
    /// can't keep up with the server.
    ///
    /// This `capacity` is applied per connection and
    /// applies globally on the connection which implies
    /// all JSON-RPC messages.
    ///
    /// For example if a subscription produces plenty of new items
    /// and the client can't keep up then no new messages are handled.
    ///
    /// If this limit is exceeded then the server will "back-off"
    /// and only accept new messages once the client reads pending messages.
    pub fn set_message_buffer_capacity(mut self, c: u32) -> Self {
        self.config = self.config.set_message_buffer_capacity(c);
        self
    }

    /// Configure a custom [`tokio::runtime::Handle`] to run the server on.
    ///
    /// Default: [`tokio::spawn`]
    pub fn custom_tokio_runtime(mut self, rt: tokio::runtime::Handle) -> Self {
        self.config = self.config.custom_tokio_runtime(rt);
        self
    }

    /// Sets the permissions for the IPC socket file.
    pub fn set_ipc_socket_permissions(mut self, permissions: Option<String>) -> Self {
        self.socket_permissions = permissions;
        self
    }

    /// Configure custom `subscription ID` provider for the server to use
    /// to when getting new subscription calls.
    ///
    /// You may choose static dispatch or dynamic dispatch because
    /// `IdProvider` is implemented for `Box<T>`.
    ///
    /// Default: [`RandomIntegerIdProvider`](reth_jasonrpeesea::RandomIntegerIdProvider).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use reth_ipc::server::Builder;
    /// use reth_jasonrpeesea::RandomStringIdProvider;
    ///
    /// // static dispatch
    /// let builder1 = Builder::default().set_id_provider(RandomStringIdProvider::new(16));
    ///
    /// // or dynamic dispatch
    /// let builder2 = Builder::default().set_id_provider(Box::new(RandomStringIdProvider::new(16)));
    /// ```
    pub fn set_id_provider<I: IdProvider + 'static>(mut self, id_provider: I) -> Self {
        self.config = self.config.set_id_provider(id_provider);
        self
    }

    /// Enable middleware that is invoked on every JSON-RPC call.
    pub fn set_rpc_middleware<T>(self, rpc_middleware: RpcServiceBuilder<T>) -> Builder<T> {
        Builder { config: self.config, socket_permissions: self.socket_permissions, rpc_middleware }
    }

    /// Finalize the configuration of the server. Consumes the [`Builder`].
    pub fn build(self, endpoint: String) -> IpcServer<RpcMiddleware> {
        IpcServer {
            endpoint,
            config: self.config,
            socket_permissions: self.socket_permissions,
            rpc_middleware: self.rpc_middleware,
        }
    }
}

#[cfg(test)]
#[expect(missing_docs)]
pub fn dummy_name() -> String {
    use rand::Rng;
    let num: u64 = rand::rng().random();
    format!(r"/tmp/my-uds-{num}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::IpcClientBuilder;
    use futures::future::{select, Either};
    use reth_jasonrpeesea::{
        client::{BatchRequestBuilder, ClientT, Error, Subscription, SubscriptionClientT},
        rpc_params, MethodResponse, PendingSubscriptionSink, Request, RpcModule,
        SubscriptionMessage, INTERNAL_ERROR_CODE, TOO_MANY_SUBSCRIPTIONS_CODE,
    };
    use reth_tracing::init_test_tracing;
    use std::future::Future;
    use tokio::sync::broadcast;
    use tokio_stream::wrappers::BroadcastStream;

    #[tokio::test]
    #[cfg(unix)]
    async fn test_ipc_socket_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let endpoint = &dummy_name();
        let perms = "0777";
        let server = Builder::default()
            .set_ipc_socket_permissions(Some(perms.to_string()))
            .build(endpoint.clone());
        let module = RpcModule::new(());
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let meta = std::fs::metadata(endpoint).unwrap();
        let perms = meta.permissions();
        assert_eq!(perms.mode() & 0o777, 0o777);
    }

    async fn pipe_from_stream_with_bounded_buffer(
        pending: PendingSubscriptionSink,
        stream: BroadcastStream<usize>,
    ) -> Result<(), Box<dyn core::error::Error + Send + Sync>> {
        let sink = pending.accept().await.unwrap();
        let closed = sink.closed();

        let mut closed = pin!(closed);
        let mut stream = pin!(stream);

        loop {
            match select(closed, stream.next()).await {
                // subscription closed or stream is closed.
                Either::Left((_, _)) | Either::Right((None, _)) => break Ok(()),

                // received new item from the stream.
                Either::Right((Some(Ok(item)), c)) => {
                    let raw_value = serde_json::value::to_raw_value(&item)?;
                    let notif = SubscriptionMessage::from(raw_value);

                    // NOTE: this will block until there a spot in the queue
                    // and you might want to do something smarter if it's
                    // critical that "the most recent item" must be sent when it is produced.
                    if sink.send(notif).await.is_err() {
                        break Ok(());
                    }

                    closed = c;
                }

                // Send back the error.
                Either::Right((Some(Err(e)), _)) => break Err(e.into()),
            }
        }
    }

    // Naive example that broadcasts the produced values to all active subscribers.
    fn produce_items(tx: broadcast::Sender<usize>) {
        for c in 1..=100 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let _ = tx.send(c);
        }
    }

    #[tokio::test]
    async fn can_set_the_max_response_body_size() {
        // init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().max_response_body_size(100).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _| "a".repeat(101)).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: Result<String, Error> = client.request("anything", rpc_params![]).await;
        assert!(response.unwrap_err().to_string().contains("Exceeded max limit of"));
    }

    #[tokio::test]
    async fn can_set_the_max_request_body_size() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().max_request_body_size(100).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _| "succeed").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: Result<String, Error> =
            client.request("anything", rpc_params!["a".repeat(101)]).await;
        assert!(response.is_err());
        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        // the raw request string is:
        //  [{"jsonrpc":"2.0","id":0,"method":"anything"},{"jsonrpc":"2.0","id":1, \
        //    "method":"anything"},{"jsonrpc":"2.0","id":2,"method":"anything"}]"
        // which is 136 bytes, more than 100 bytes.
        let response = client.batch_request::<String>(batch_request_builder).await;
        assert!(response.is_err());
    }

    #[tokio::test]
    async fn can_set_max_connections() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().max_connections(2).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _| "succeed").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client1 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let client2 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let client3 = IpcClientBuilder::default().build(endpoint).await.unwrap();

        let response1: Result<String, Error> = client1.request("anything", rpc_params![]).await;
        let response2: Result<String, Error> = client2.request("anything", rpc_params![]).await;
        let response3: Result<String, Error> = client3.request("anything", rpc_params![]).await;

        assert!(response1.is_ok());
        assert!(response2.is_ok());
        // Third connection is rejected
        assert!(response3.is_err());

        // Decrement connection count
        drop(client2);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Can connect again
        let client4 = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response4: Result<String, Error> = client4.request("anything", rpc_params![]).await;
        assert!(response4.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_pending_call_aborted_on_disconnect() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let (started_tx, started_rx) = oneshot::channel::<()>();
        let (dropped_tx, dropped_rx) = oneshot::channel::<()>();
        let mut module = RpcModule::new(std::sync::Mutex::new(Some((started_tx, dropped_tx))));
        module
            .register_async_method("hang", |_, ctx| async move {
                // `dropped_tx` is dropped together with the call
                let (started_tx, _dropped_tx) = ctx.lock().unwrap().take().unwrap();
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
                "unreachable"
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        tokio::select! {
            _ = client.request::<String, _>("hang", rpc_params![]) => panic!("call completed"),
            _ = started_rx => {}
        }
        drop(client);

        let dropped = tokio::time::timeout(std::time::Duration::from_secs(5), dropped_rx).await;
        assert!(dropped.is_ok(), "call kept running after the connection closed");
    }

    #[tokio::test]
    async fn test_panicking_call_returns_internal_error() {
        init_test_tracing();

        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module
            .register_async_method("maybe_panic", |params, _| async move {
                assert!(!params.one::<bool>().unwrap(), "requested panic");
                "ok"
            })
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let err = client.request::<String, _>("maybe_panic", rpc_params![true]).await.unwrap_err();
        assert!(matches!(&err, Error::Call(err) if err.code() == INTERNAL_ERROR_CODE), "{err:?}");

        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("maybe_panic", rpc_params![true]);
        let _ = batch_request_builder.insert("maybe_panic", rpc_params![false]);
        let responses = client.batch_request::<String>(batch_request_builder).await.unwrap();
        assert!(matches!(&responses[0], Err(err) if err.code() == INTERNAL_ERROR_CODE));
        assert_eq!(responses[1].as_deref(), Ok("ok"));
    }

    #[tokio::test]
    async fn test_rpc_request() {
        init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"jsonrpc":"2.0","id":83,"result":"0x7a69"}"#;
        module.register_method("eth_chainId", move |_, _| msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: String = client.request("eth_chainId", rpc_params![]).await.unwrap();
        assert_eq!(response, msg);
    }

    #[tokio::test]
    async fn test_batch_request() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        module.register_method("anything", |_, _| "ok").unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let mut batch_request_builder = BatchRequestBuilder::new();
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let _ = batch_request_builder.insert("anything", rpc_params![]);
        let result = client
            .batch_request(batch_request_builder)
            .await
            .unwrap()
            .into_iter()
            .collect::<Result<Vec<String>, _>>()
            .unwrap();
        assert_eq!(result, vec!["ok", "ok", "ok"]);
    }

    #[tokio::test]
    async fn test_ipc_modules() {
        reth_tracing::init_test_tracing();
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let mut module = RpcModule::new(());
        let msg = r#"{"admin":"1.0","debug":"1.0","engine":"1.0","eth":"1.0","ethash":"1.0","miner":"1.0","net":"1.0","rpc":"1.0","txpool":"1.0","web3":"1.0"}"#;
        module.register_method("rpc_modules", move |_, _| msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let response: String = client.request("rpc_modules", rpc_params![]).await.unwrap();
        assert_eq!(response, msg);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_rpc_subscription() {
        let endpoint = &dummy_name();
        let server = Builder::default().build(endpoint.clone());
        let (tx, _rx) = broadcast::channel::<usize>(16);

        let mut module = RpcModule::new(tx.clone());
        std::thread::spawn(move || produce_items(tx));

        module
            .register_subscription(
                "subscribe_hello",
                "s_hello",
                "unsubscribe_hello",
                |_, pending, tx| async move {
                    let rx = tx.subscribe();
                    let stream = BroadcastStream::new(rx);
                    pipe_from_stream_with_bounded_buffer(pending, stream).await?;
                    Ok(())
                },
            )
            .unwrap();

        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let sub: Subscription<usize> =
            client.subscribe("subscribe_hello", rpc_params![], "unsubscribe_hello").await.unwrap();

        let items = sub.take(16).collect::<Vec<_>>().await;
        assert_eq!(items.len(), 16);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_max_subscriptions_per_connection() {
        let endpoint = &dummy_name();
        let server = Builder::default().max_subscriptions_per_connection(1).build(endpoint.clone());
        let mut module = RpcModule::new(());
        module
            .register_subscription(
                "subscribe_hello",
                "s_hello",
                "unsubscribe_hello",
                |_, pending, _| async move {
                    if let Ok(sink) = pending.accept().await {
                        sink.closed().await;
                    }
                },
            )
            .unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let _sub: Subscription<usize> =
            client.subscribe("subscribe_hello", rpc_params![], "unsubscribe_hello").await.unwrap();
        let err = client
            .subscribe::<usize, _>("subscribe_hello", rpc_params![], "unsubscribe_hello")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::Call(err) if err.code() == TOO_MANY_SUBSCRIPTIONS_CODE),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_rpc_middleware() {
        #[derive(Clone)]
        struct ModifyRequestIf<S>(S);

        impl<S: RpcServiceT> RpcServiceT for ModifyRequestIf<S> {
            fn call(&self, mut req: Request) -> impl Future<Output = MethodResponse> + Send {
                // Re-direct all calls that isn't `say_hello` to `say_goodbye`
                if req.method == "say_hello" {
                    req.method = "say_goodbye".into();
                } else if req.method == "say_goodbye" {
                    req.method = "say_hello".into();
                }

                self.0.call(req)
            }
        }

        reth_tracing::init_test_tracing();
        let endpoint = &dummy_name();

        let rpc_middleware = RpcServiceBuilder::new().layer_fn(ModifyRequestIf);
        let server = Builder::default().set_rpc_middleware(rpc_middleware).build(endpoint.clone());

        let mut module = RpcModule::new(());
        let goodbye_msg = r#"{"jsonrpc":"2.0","id":1,"result":"goodbye"}"#;
        let hello_msg = r#"{"jsonrpc":"2.0","id":2,"result":"hello"}"#;
        module.register_method("say_hello", move |_, _| hello_msg).unwrap();
        module.register_method("say_goodbye", move |_, _| goodbye_msg).unwrap();
        let handle = server.start(module).await.unwrap();
        tokio::spawn(handle.stopped());

        let client = IpcClientBuilder::default().build(endpoint).await.unwrap();
        let say_hello_response: String = client.request("say_hello", rpc_params![]).await.unwrap();
        let say_goodbye_response: String =
            client.request("say_goodbye", rpc_params![]).await.unwrap();

        assert_eq!(say_hello_response, goodbye_msg);
        assert_eq!(say_goodbye_response, hello_msg);
    }
}
