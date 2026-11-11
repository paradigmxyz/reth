use super::{
    decode, decode_batch, write_batch, write_notification, write_request, BatchRequestBuilder,
    BoxError, ClientT, Error, RawResponse, SubscriptionClientT, ToRpcParams,
};
use crate::{ErrorObject, SubscriptionId};
use futures_util::{stream, Sink, SinkExt, Stream, StreamExt};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::{
    marker::PhantomData,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{
    mpsc::{self, error::TrySendError},
    oneshot, Semaphore, SemaphorePermit,
};

type CallResult = Result<Box<RawValue>, ErrorObject>;
/// The outer error means the server rejected the whole message the call was part of.
type CallMessageResult = Result<CallResult, ErrorObject>;
type SubscribeResult =
    Result<(SubscriptionId, mpsc::Receiver<Box<RawValue>>, Arc<AtomicBool>), ErrorObject>;

/// Builds a [`Client`] over a message transport.
#[derive(Clone, Copy, Debug)]
pub struct ClientBuilder {
    request_timeout: Duration,
    subscription_buffer: usize,
    max_concurrent_requests: usize,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            subscription_buffer: 1024,
            max_concurrent_requests: 256,
        }
    }
}

impl ClientBuilder {
    /// Sets the request timeout. Default is 60 seconds.
    pub const fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Sets the number of notifications buffered per subscription. Default is 1024.
    ///
    /// A subscription that falls further behind is closed.
    pub const fn max_buffer_capacity_per_subscription(mut self, capacity: usize) -> Self {
        self.subscription_buffer = capacity;
        self
    }

    /// Sets the maximum number of requests, batches and subscription calls awaiting a response.
    /// Default is 256.
    ///
    /// Further calls wait until an earlier one completes.
    pub const fn max_concurrent_requests(mut self, max: usize) -> Self {
        self.max_concurrent_requests = max;
        self
    }

    /// Creates a client that reads messages from `reader` and writes them to `writer`.
    ///
    /// Spawns a task that drives the connection until the client and all its subscriptions are
    /// dropped, or the transport closes.
    pub fn build<R, T, E, W>(self, reader: R, writer: W) -> Client
    where
        R: Stream<Item = Result<T, E>> + Send + Unpin + 'static,
        T: AsRef<[u8]> + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        W: Sink<String> + Send + Unpin + 'static,
        W::Error: Into<BoxError> + Send + 'static,
    {
        self.build_with_keepalive(reader, writer, stream::pending())
    }

    /// Like [`Self::build`], but also writes the messages yielded by `keepalive`.
    pub(super) fn build_with_keepalive<R, T, E, W, M, K>(
        self,
        reader: R,
        writer: W,
        keepalive: K,
    ) -> Client
    where
        R: Stream<Item = Result<T, E>> + Send + Unpin + 'static,
        T: AsRef<[u8]> + Send + 'static,
        E: Into<BoxError> + Send + 'static,
        W: Sink<M> + Send + Unpin + 'static,
        W::Error: Into<BoxError> + Send + 'static,
        M: From<String> + Send + 'static,
        K: Stream<Item = M> + Send + Unpin + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(drive(reader, writer, keepalive, rx, self.subscription_buffer));
        Client {
            tx,
            next_id: Arc::default(),
            request_timeout: self.request_timeout,
            requests: Arc::new(Semaphore::new(
                self.max_concurrent_requests.min(Semaphore::MAX_PERMITS),
            )),
        }
    }
}

/// A client over a message transport, such as `WebSocket` or IPC.
#[derive(Clone, Debug)]
pub struct Client {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
    request_timeout: Duration,
    requests: Arc<Semaphore>,
}

impl Client {
    /// Returns `true` if the connection is still open.
    pub fn is_connected(&self) -> bool {
        !self.tx.is_closed()
    }

    /// Resolves once the connection is closed.
    pub async fn on_disconnect(&self) {
        self.tx.closed().await
    }

    /// Returns the request timeout.
    pub const fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    fn next_ids(&self, n: u64) -> u64 {
        self.next_id.fetch_add(n, Ordering::Relaxed)
    }

    fn send(&self, json: String, pending: Vec<(u64, Pending)>) -> Result<(), Error> {
        self.tx.send(Command::Send { json, pending }).map_err(|_| Error::Closed)
    }

    async fn acquire(&self) -> Result<SemaphorePermit<'_>, Error> {
        self.requests.acquire().await.map_err(|_| Error::Closed)
    }

    async fn wait<T>(&self, rx: oneshot::Receiver<T>) -> Result<T, Error> {
        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => Err(Error::RequestTimeout),
        }
    }
}

impl ClientT for Client {
    async fn request<R, P>(&self, method: &str, params: P) -> Result<R, Error>
    where
        R: DeserializeOwned,
        P: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let _permit = self.acquire().await?;
        let id = self.next_ids(1);
        let mut json = String::new();
        write_request(&mut json, id, method, params.as_deref());
        let (tx, rx) = oneshot::channel();
        self.send(json, vec![(id, Pending::Call(tx))])?;
        decode(self.wait(rx).await?.map_err(Error::Call)?)
    }

    async fn batch_request<R>(
        &self,
        batch: BatchRequestBuilder,
    ) -> Result<Vec<Result<R, ErrorObject>>, Error>
    where
        R: DeserializeOwned,
    {
        let _permit = self.acquire().await?;
        let first_id = self.next_ids(batch.0.len() as u64);
        let json = write_batch(&batch, first_id);
        let (pending, receivers): (Vec<_>, Vec<_>) = (first_id..)
            .take(batch.0.len())
            .map(|id| {
                let (tx, rx) = oneshot::channel();
                ((id, Pending::Call(tx)), rx)
            })
            .unzip();
        self.send(json, pending)?;
        let mut results = Vec::with_capacity(receivers.len());
        for rx in receivers {
            results.push(self.wait(rx).await?.map_err(Error::Call)?);
        }
        decode_batch(results)
    }

    async fn notification<P>(&self, method: &str, params: P) -> Result<(), Error>
    where
        P: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let mut json = String::new();
        write_notification(&mut json, method, params.as_deref());
        self.send(json, Vec::new())
    }
}

impl SubscriptionClientT for Client {
    async fn subscribe<N, P>(
        &self,
        subscribe: &str,
        params: P,
        unsubscribe: &str,
    ) -> Result<Subscription<N>, Error>
    where
        N: DeserializeOwned,
        P: ToRpcParams + Send,
    {
        let params = params.to_rpc_params()?;
        let permit = self.acquire().await?;
        let id = self.next_ids(1);
        let mut json = String::new();
        write_request(&mut json, id, subscribe, params.as_deref());
        let (tx, rx) = oneshot::channel();
        self.send(json, vec![(id, Pending::Subscribe(tx))])?;
        let (sub_id, rx, lagged) = self.wait(rx).await?.map_err(Error::Call)?;
        drop(permit);
        Ok(Subscription {
            rx,
            lagged,
            sub_id,
            unsubscribe: Some(unsubscribe.to_owned()),
            client: self.clone(),
            _marker: PhantomData,
        })
    }
}

/// A stream of subscription notifications.
///
/// Dropping it unsubscribes.
///
/// The client buffers notifications until they are read, and closes the subscription once the
/// buffer is full. [`Subscription::close_reason`] tells why it was closed.
#[derive(Debug)]
pub struct Subscription<T> {
    rx: mpsc::Receiver<Box<RawValue>>,
    lagged: Arc<AtomicBool>,
    sub_id: SubscriptionId,
    unsubscribe: Option<String>,
    client: Client,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Subscription<T> {
    /// Returns the subscription id.
    pub const fn id(&self) -> &SubscriptionId {
        &self.sub_id
    }

    /// Returns the next notification, or `None` once the subscription is closed.
    pub async fn next(&mut self) -> Option<Result<T, serde_json::Error>>
    where
        T: DeserializeOwned,
    {
        let item = self.rx.recv().await?;
        Some(serde_json::from_str(item.get()))
    }

    /// Returns why the subscription was closed, or `None` if it is open.
    ///
    /// Notifications received before it was closed can still be read.
    pub fn close_reason(&self) -> Option<SubscriptionCloseReason> {
        if self.lagged.load(Ordering::Relaxed) {
            Some(SubscriptionCloseReason::Lagged)
        } else if self.rx.is_closed() {
            Some(SubscriptionCloseReason::ConnectionClosed)
        } else {
            None
        }
    }

    /// Unsubscribes and waits for the server to confirm.
    pub async fn unsubscribe(mut self) -> Result<(), Error> {
        let Some(method) = self.unsubscribe.take() else { return Ok(()) };
        let _ =
            self.client.tx.send(Command::Unsubscribe { json: None, sub_id: self.sub_id.clone() });
        self.client.request::<bool, _>(&method, crate::rpc_params![&self.sub_id]).await.map(drop)
    }
}

impl<T: DeserializeOwned> Stream for Subscription<T> {
    type Item = Result<T, serde_json::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|item| item.map(|item| serde_json::from_str(item.get())))
    }
}

impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        if let Some(method) = self.unsubscribe.take() {
            let mut json = String::new();
            let params = serde_json::value::to_raw_value(&[&self.sub_id]).ok();
            write_request(&mut json, self.client.next_ids(1), &method, params.as_deref());
            let _ = self
                .client
                .tx
                .send(Command::Unsubscribe { json: Some(json), sub_id: self.sub_id.clone() });
        }
    }
}

/// Why a [`Subscription`] was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionCloseReason {
    /// The connection was closed.
    ConnectionClosed,
    /// The subscription was not read fast enough and its buffer filled up.
    Lagged,
}

#[derive(Debug)]
enum Command {
    /// Sends a message, registering the given pending calls.
    Send { json: String, pending: Vec<(u64, Pending)> },
    /// Closes a subscription, sending the unsubscribe call if any.
    Unsubscribe { json: Option<String>, sub_id: SubscriptionId },
}

#[derive(Debug)]
enum Pending {
    Call(oneshot::Sender<CallMessageResult>),
    Subscribe(oneshot::Sender<SubscribeResult>),
}

impl Pending {
    fn fail(self, err: ErrorObject) {
        match self {
            Self::Call(tx) => drop(tx.send(Err(err))),
            Self::Subscribe(tx) => drop(tx.send(Err(err))),
        }
    }

    /// Returns `true` if the caller stopped waiting, for example after a timeout.
    fn is_closed(&self) -> bool {
        match self {
            Self::Call(tx) => tx.is_closed(),
            Self::Subscribe(tx) => tx.is_closed(),
        }
    }
}

#[derive(Default)]
struct State {
    /// Pending calls by id, with the id of the first call sent in the same message.
    pending: FxHashMap<u64, (u64, Pending)>,
    /// Number of pending calls at which to remove the ones nobody waits for.
    prune_at: usize,
    /// Notification senders and lag flags of open subscriptions.
    subscriptions: FxHashMap<SubscriptionId, (mpsc::Sender<Box<RawValue>>, Arc<AtomicBool>)>,
    subscription_buffer: usize,
}

async fn drive<R, T, E, W, M, K>(
    mut reader: R,
    mut writer: W,
    mut keepalive: K,
    mut commands: mpsc::UnboundedReceiver<Command>,
    subscription_buffer: usize,
) where
    R: Stream<Item = Result<T, E>> + Unpin,
    T: AsRef<[u8]>,
    E: Into<BoxError>,
    W: Sink<M> + Unpin,
    W::Error: Into<BoxError>,
    M: From<String>,
    K: Stream<Item = M> + Unpin,
{
    let mut state = State { subscription_buffer, ..Default::default() };
    loop {
        let msg = tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Send { json, pending }) => {
                    state.add_pending(pending);
                    M::from(json)
                }
                Some(Command::Unsubscribe { json, sub_id }) => {
                    state.subscriptions.remove(&sub_id);
                    let Some(json) = json else { continue };
                    M::from(json)
                }
                None => break,
            },
            Some(msg) = keepalive.next() => msg,
            msg = reader.next() => {
                match msg {
                    Some(Ok(msg)) => state.handle(msg.as_ref()),
                    Some(Err(err)) => {
                        tracing::debug!(target: "rpc::jsonrpc", err = %err.into(), "failed to receive response");
                        break
                    }
                    None => break,
                }
                continue
            }
        };
        if let Err(err) = writer.send(msg).await {
            tracing::debug!(target: "rpc::jsonrpc", err = %err.into(), "failed to send request");
            break
        }
    }
    // Fail pending calls and signal the disconnect before closing the transport.
    drop(commands);
    drop(state);
    let _ = writer.close().await;
}

impl State {
    fn add_pending(&mut self, pending: Vec<(u64, Pending)>) {
        let Some(&(message, _)) = pending.first() else { return };
        self.pending.extend(pending.into_iter().map(|(id, pending)| (id, (message, pending))));
        if self.pending.len() >= self.prune_at {
            self.pending.retain(|_, (_, pending)| !pending.is_closed());
            self.prune_at = (self.pending.len() * 2).max(64);
        }
    }

    fn handle(&mut self, msg: &[u8]) {
        if msg.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'[') {
            match serde_json::from_slice::<Vec<RawResponse<'_>>>(msg) {
                Ok(responses) => {
                    for response in responses {
                        self.handle_one(response);
                    }
                }
                Err(err) => tracing::debug!(target: "rpc::jsonrpc", %err, "invalid batch response"),
            }
            return
        }
        match serde_json::from_slice::<RawResponse<'_>>(msg) {
            Ok(response) => self.handle_one(response),
            Err(err) => tracing::debug!(target: "rpc::jsonrpc", %err, "invalid response"),
        }
    }

    fn handle_one(&mut self, response: RawResponse<'_>) {
        if response.method.is_some() &&
            let Some(params) = response.params
        {
            let Some((tx, lagged)) = self.subscriptions.get(&params.subscription) else { return };
            // Close subscriptions that are dropped or lag behind.
            if let Err(err) = tx.try_send(params.result.to_owned()) {
                if matches!(err, TrySendError::Full(_)) {
                    lagged.store(true, Ordering::Relaxed);
                }
                self.subscriptions.remove(&params.subscription);
            }
            return
        }

        let Some(id) = response.id else {
            // The server could not tell which request failed, which is only clear if all pending
            // calls were sent in one message. Otherwise they are left to time out rather than
            // failing calls that may succeed.
            let mut messages = self.pending.values().map(|(message, _)| *message);
            if let Some(err) = response.error &&
                let Some(message) = messages.next() &&
                messages.all(|m| m == message)
            {
                self.pending.drain().for_each(|(_, (_, pending))| pending.fail(err.clone()));
            } else {
                tracing::debug!(target: "rpc::jsonrpc", "response without id");
            }
            return
        };
        let Some((_, pending)) = self.pending.remove(&id) else { return };
        match pending {
            Pending::Call(tx) => {
                let _ = tx.send(Ok(response.into_result()));
            }
            Pending::Subscribe(tx) => {
                let result = response.into_result().and_then(|result| {
                    serde_json::from_str::<SubscriptionId>(result.get())
                        .map_err(|_| crate::ErrorCode::InvalidRequest.into())
                });
                let result = result.map(|sub_id| {
                    let (sub_tx, sub_rx) = mpsc::channel(self.subscription_buffer.max(1));
                    let lagged = Arc::default();
                    self.subscriptions.insert(sub_id.clone(), (sub_tx, Arc::clone(&lagged)));
                    (sub_id, sub_rx, lagged)
                });
                if let Err(Ok((sub_id, ..))) = tx.send(result) {
                    self.subscriptions.remove(&sub_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        connection::serve_connection, stop_channel, Extensions, RpcModule, RpcServiceBuilder,
        ServerConfig,
    };
    use bytes::Bytes;
    use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
    use futures_util::StreamExt;
    use serde_json::json;

    /// Connects a client to a server over in-memory channels.
    fn connect(module: RpcModule) -> Client {
        let (client_tx, server_rx) = futures::channel::mpsc::unbounded::<String>();
        let (server_tx, client_rx) = futures::channel::mpsc::unbounded::<String>();
        let (stop, handle) = stop_channel();
        tokio::spawn(async move {
            let reader = server_rx.map(Bytes::from);
            serve_connection(
                reader,
                server_tx,
                module,
                &RpcServiceBuilder::new(),
                &ServerConfig::default(),
                stop,
                Extensions::new(),
            )
            .await;
            drop(handle);
        });
        ClientBuilder::default().build(client_rx.map(Ok::<_, BoxError>), client_tx)
    }

    /// The server side of [`connect_raw`].
    struct RawServer {
        rx: UnboundedReceiver<String>,
        tx: UnboundedSender<String>,
    }

    impl RawServer {
        /// Receives a request and returns its id.
        async fn recv_id(&mut self) -> u64 {
            let msg = self.rx.next().await.unwrap();
            serde_json::from_str::<serde_json::Value>(&msg).unwrap()["id"].as_u64().unwrap()
        }

        fn send(&self, msg: serde_json::Value) {
            self.tx.unbounded_send(msg.to_string()).unwrap();
        }

        fn respond(&self, id: u64, result: u64) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }

        fn notify(&self, subscription: u64, result: u64) {
            self.send(json!({
                "jsonrpc": "2.0",
                "method": "notif",
                "params": { "subscription": subscription, "result": result },
            }));
        }
    }

    /// Connects a client to in-memory channels that the test answers.
    fn connect_raw(builder: ClientBuilder) -> (Client, RawServer) {
        let (client_tx, rx) = futures::channel::mpsc::unbounded::<String>();
        let (tx, client_rx) = futures::channel::mpsc::unbounded::<String>();
        (builder.build(client_rx.map(Ok::<_, BoxError>), client_tx), RawServer { rx, tx })
    }

    async fn subscribe(client: &Client, server: &mut RawServer, sub_id: u64) -> Subscription<u64> {
        let (sub, ()) =
            tokio::join!(client.subscribe("sub", crate::rpc_params![], "unsub"), async {
                let id = server.recv_id().await;
                server.respond(id, sub_id);
            });
        sub.unwrap()
    }

    #[tokio::test]
    async fn request_batch_subscribe() {
        let mut module = RpcModule::new();
        module
            .register_method("add", |params, _| params.parse::<(u64, u64)>().map(|(a, b)| a + b))
            .unwrap();
        module
            .register_subscription("sub", "notif", "unsub", |params, pending, _| async move {
                let n = params.one::<u64>()?;
                let sink = pending.accept().await?;
                for i in 0..n {
                    sink.send(&i).await?;
                }
                sink.closed().await;
                Ok(())
            })
            .unwrap();
        let client = connect(module);

        assert_eq!(client.request::<u64, _>("add", crate::rpc_params![1, 2]).await.unwrap(), 3);
        let err = client.request::<u64, _>("nope", crate::rpc_params![]).await.unwrap_err();
        assert!(matches!(err, Error::Call(err) if err.code() == crate::METHOD_NOT_FOUND_CODE));

        let mut batch = BatchRequestBuilder::new();
        batch.insert("add", crate::rpc_params![1, 1]).unwrap();
        batch.insert("nope", crate::rpc_params![]).unwrap();
        let results = client.batch_request::<u64>(batch).await.unwrap();
        assert_eq!(results[0], Ok(2));
        assert_eq!(results[1].as_ref().unwrap_err().code(), crate::METHOD_NOT_FOUND_CODE);

        let sub = client.subscribe::<u64, _>("sub", crate::rpc_params![3], "unsub").await.unwrap();
        let items = sub.take(3).map(Result::unwrap).collect::<Vec<_>>().await;
        assert_eq!(items, [0, 1, 2]);

        let sub = client.subscribe::<u64, _>("sub", crate::rpc_params![0], "unsub").await.unwrap();
        sub.unsubscribe().await.unwrap();

        let err =
            client.subscribe::<u64, _>("sub", crate::rpc_params!["x"], "unsub").await.unwrap_err();
        assert!(matches!(err, Error::Call(err) if err.code() == crate::INTERNAL_ERROR_CODE));
    }

    #[test]
    fn prune_abandoned_calls() {
        let calls = |ids: std::ops::Range<u64>| -> (Vec<_>, Vec<_>) {
            ids.map(|id| {
                let (tx, rx) = oneshot::channel();
                ((id, Pending::Call(tx)), rx)
            })
            .unzip()
        };
        let mut state = State::default();
        let (pending, abandoned) = calls(0..100);
        state.add_pending(pending);
        drop(abandoned);
        let (pending, _waiting) = calls(100..200);
        state.add_pending(pending);
        assert_eq!(state.pending.len(), 100);
        assert!(state.pending.keys().all(|id| *id >= 100));
    }

    #[test]
    fn response_without_id() {
        let error =
            br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#;
        let mut state = State::default();
        let (tx1, mut rx1) = oneshot::channel();
        let (tx2, mut rx2) = oneshot::channel();
        state.add_pending(vec![(1, Pending::Call(tx1))]);
        state.add_pending(vec![(2, Pending::Call(tx2))]);
        state.handle(error);
        assert!(rx1.try_recv().is_err() && rx2.try_recv().is_err());
        assert_eq!(state.pending.len(), 2);

        state.pending.remove(&1);
        state.handle(error);
        assert_eq!(rx2.try_recv().unwrap().unwrap_err().code(), -32700);
        assert!(state.pending.is_empty());

        let (tx3, mut rx3) = oneshot::channel();
        let (tx4, mut rx4) = oneshot::channel();
        state.add_pending(vec![(3, Pending::Call(tx3)), (4, Pending::Call(tx4))]);
        state.handle(error);
        assert!(rx3.try_recv().unwrap().is_err() && rx4.try_recv().unwrap().is_err());
    }

    #[tokio::test]
    async fn notification_and_disconnect() {
        let (client, mut server) = connect_raw(ClientBuilder::default());
        assert_eq!(client.request_timeout(), Duration::from_secs(60));

        client.notification("n", crate::rpc_params![1]).await.unwrap();
        assert_eq!(
            server.rx.next().await.unwrap(),
            r#"{"jsonrpc":"2.0","method":"n","params":[1]}"#
        );

        assert!(client.is_connected());
        drop(server);
        tokio::time::timeout(Duration::from_secs(5), client.on_disconnect()).await.unwrap();
        assert!(!client.is_connected());
        let err = client.notification("n", crate::rpc_params![]).await.unwrap_err();
        assert!(matches!(err, Error::Closed));
    }

    #[tokio::test]
    async fn max_concurrent_requests() {
        let (client, mut server) = connect_raw(ClientBuilder::default().max_concurrent_requests(1));
        let requests = [client.clone(), client].map(|client| {
            tokio::spawn(async move { client.request::<u64, _>("m", crate::rpc_params![]).await })
        });

        let first = server.recv_id().await;
        assert!(tokio::time::timeout(Duration::from_millis(50), server.rx.next()).await.is_err());
        server.respond(first, 1);
        let second = server.recv_id().await;
        server.respond(second, 1);
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap(), 1);
        }
    }

    #[tokio::test]
    async fn subscription_close_reason() {
        let (client, mut server) =
            connect_raw(ClientBuilder::default().max_buffer_capacity_per_subscription(1));
        let mut lagging = subscribe(&client, &mut server, 1).await;
        let mut closed = subscribe(&client, &mut server, 2).await;
        assert_eq!(lagging.close_reason(), None);

        server.notify(1, 10);
        server.notify(1, 11);
        // The response arrives after the notifications were handled.
        let (result, ()) =
            tokio::join!(client.request::<u64, _>("m", crate::rpc_params![]), async {
                let id = server.recv_id().await;
                server.respond(id, 0);
            });
        assert_eq!(result.unwrap(), 0);
        assert_eq!(lagging.close_reason(), Some(SubscriptionCloseReason::Lagged));
        assert_eq!(lagging.next().await.unwrap().unwrap(), 10);
        assert!(lagging.next().await.is_none());

        assert_eq!(closed.close_reason(), None);
        drop(server);
        assert!(closed.next().await.is_none());
        assert_eq!(closed.close_reason(), Some(SubscriptionCloseReason::ConnectionClosed));
    }
}
