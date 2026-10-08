use super::{
    decode, decode_batch, write_batch, write_request, BatchRequestBuilder, BoxError, ClientT,
    Error, RawResponse, SubscriptionClientT, ToRpcParams,
};
use crate::{ErrorObject, SubscriptionId};
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::{
    marker::PhantomData,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

type CallResult = Result<Box<RawValue>, ErrorObject>;
/// The outer error means the server rejected the whole message the call was part of.
type CallMessageResult = Result<CallResult, ErrorObject>;
type SubscribeResult = Result<(SubscriptionId, mpsc::Receiver<Box<RawValue>>), ErrorObject>;

/// Builds a [`Client`] over a message transport.
#[derive(Clone, Copy, Debug)]
pub struct ClientBuilder {
    request_timeout: Duration,
    subscription_buffer: usize,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self { request_timeout: Duration::from_secs(60), subscription_buffer: 1024 }
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
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(drive(reader, writer, rx, self.subscription_buffer));
        Client { tx, next_id: Arc::default(), request_timeout: self.request_timeout }
    }
}

/// A client over a message transport, such as `WebSocket` or IPC.
#[derive(Clone, Debug)]
pub struct Client {
    tx: mpsc::UnboundedSender<Command>,
    next_id: Arc<AtomicU64>,
    request_timeout: Duration,
}

impl Client {
    /// Returns `true` if the connection is still open.
    pub fn is_connected(&self) -> bool {
        !self.tx.is_closed()
    }

    fn next_ids(&self, n: u64) -> u64 {
        self.next_id.fetch_add(n, Ordering::Relaxed)
    }

    fn send(&self, json: String, pending: Vec<(u64, Pending)>) -> Result<(), Error> {
        self.tx.send(Command::Send { json, pending }).map_err(|_| Error::Closed)
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
        let id = self.next_ids(1);
        let mut json = String::new();
        write_request(&mut json, id, subscribe, params.as_deref());
        let (tx, rx) = oneshot::channel();
        self.send(json, vec![(id, Pending::Subscribe(tx))])?;
        let (sub_id, rx) = self.wait(rx).await?.map_err(Error::Call)?;
        Ok(Subscription {
            rx,
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
#[derive(Debug)]
pub struct Subscription<T> {
    rx: mpsc::Receiver<Box<RawValue>>,
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
}

#[derive(Default)]
struct State {
    pending: FxHashMap<u64, Pending>,
    subscriptions: FxHashMap<SubscriptionId, mpsc::Sender<Box<RawValue>>>,
    subscription_buffer: usize,
}

async fn drive<R, T, E, W>(
    mut reader: R,
    mut writer: W,
    mut commands: mpsc::UnboundedReceiver<Command>,
    subscription_buffer: usize,
) where
    R: Stream<Item = Result<T, E>> + Unpin,
    T: AsRef<[u8]>,
    E: Into<BoxError>,
    W: Sink<String> + Unpin,
    W::Error: Into<BoxError>,
{
    let mut state = State { subscription_buffer, ..Default::default() };
    loop {
        tokio::select! {
            command = commands.recv() => {
                let json = match command {
                    Some(Command::Send { json, pending }) => {
                        state.pending.extend(pending);
                        json
                    }
                    Some(Command::Unsubscribe { json, sub_id }) => {
                        state.subscriptions.remove(&sub_id);
                        let Some(json) = json else { continue };
                        json
                    }
                    None => break,
                };
                if let Err(err) = writer.send(json).await {
                    tracing::debug!(target: "rpc::jsonrpc", err = %err.into(), "failed to send request");
                    break
                }
            }
            msg = reader.next() => match msg {
                Some(Ok(msg)) => state.handle(msg.as_ref()),
                Some(Err(err)) => {
                    tracing::debug!(target: "rpc::jsonrpc", err = %err.into(), "failed to receive response");
                    break
                }
                None => break,
            },
        }
    }
    let _ = writer.close().await;
}

impl State {
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
            let Some(tx) = self.subscriptions.get(&params.subscription) else { return };
            // Close subscriptions that are dropped or lag behind.
            if tx.try_send(params.result.to_owned()).is_err() {
                self.subscriptions.remove(&params.subscription);
            }
            return
        }

        let Some(id) = response.id else {
            // The server could not tell which request failed, so fail all of them.
            if let Some(err) = response.error {
                self.pending.drain().for_each(|(_, pending)| pending.fail(err.clone()));
            }
            return
        };
        let Some(pending) = self.pending.remove(&id) else { return };
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
                    self.subscriptions.insert(sub_id.clone(), sub_tx);
                    (sub_id, sub_rx)
                });
                if let Err(Ok((sub_id, _))) = tx.send(result) {
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
        connection::serve_connection, stop_channel, RpcModule, RpcServiceBuilder, ServerConfig,
    };
    use bytes::Bytes;
    use futures_util::StreamExt;

    /// Connects a client to a server over in-memory channels.
    fn connect(module: RpcModule<()>) -> Client {
        let (client_tx, server_rx) = futures::channel::mpsc::unbounded::<String>();
        let (server_tx, client_rx) = futures::channel::mpsc::unbounded::<String>();
        let (stop, handle) = stop_channel();
        tokio::spawn(async move {
            let reader = server_rx.map(Bytes::from);
            serve_connection(
                reader,
                server_tx,
                module.into(),
                &RpcServiceBuilder::new(),
                &ServerConfig::default(),
                stop,
            )
            .await;
            drop(handle);
        });
        ClientBuilder::default().build(client_rx.map(Ok::<_, BoxError>), client_tx)
    }

    #[tokio::test]
    async fn request_batch_subscribe() {
        let mut module = RpcModule::new(());
        module
            .register_method("add", |params, _| params.parse::<(u64, u64)>().map(|(a, b)| a + b))
            .unwrap();
        module
            .register_subscription("sub", "notif", "unsub", |params, pending, _| async move {
                let n = params.one::<u64>()?;
                let sink = pending.accept().await?;
                for i in 0..n {
                    let msg = crate::SubscriptionMessage::new(
                        sink.method_name(),
                        sink.subscription_id(),
                        &i,
                    )?;
                    sink.send(msg).await?;
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
}
