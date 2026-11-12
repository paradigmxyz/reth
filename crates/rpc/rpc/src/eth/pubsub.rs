//! `eth_` `PubSub` RPC handler implementation

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata, TransactionReceiptsParams,
    },
    Filter,
};
use futures::StreamExt;
use jsonrpsee::{
    core::server::TrySendError,
    server::{ConnectionId, SubscriptionMessage},
    types::ErrorObject,
    PendingSubscriptionSink, SubscriptionSink,
};
use parking_lot::Mutex;
use reth_chain_state::CanonStateSubscriptions;
use reth_network_api::NetworkInfo;
use reth_rpc_convert::RpcHeader;
use reth_rpc_eth_api::{
    helpers::EthSubscriptions, pubsub::EthPubSubApiServer, RpcConvert, RpcLog, RpcNodeCore,
    RpcTransaction,
};
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::BlockNumReader;
use reth_tasks::Runtime;
use reth_transaction_pool::{NewTransactionEvent, TransactionPool};
use serde::Serialize;
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};
use tracing::{debug, error};

/// Upper bound on the serialized notification bytes `eth_subscribe` may have queued, but not yet
/// written, on a single connection.
///
/// jsonrpsee queues up to 1024 messages per connection whatever their size, so without this a
/// client that stops reading makes the node retain up to 1024 full notifications per connection.
const MAX_QUEUED_NOTIFICATION_BYTES_PER_CONNECTION: usize = 16 * 1024 * 1024;

/// How long a subscription waits for its connection to drain before it is closed.
const STALLED_SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a subscription that is over its connection's budget checks whether it drained.
const BACKLOG_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Eth>>,
}

// === impl EthPubSub ===

impl<Eth> EthPubSub<Eth> {
    /// Creates a new, shareable instance.
    pub fn new(eth_api: Eth, subscription_task_spawner: Runtime) -> Self {
        let inner = EthPubSubInner { eth_api, subscription_task_spawner, backlogs: Arc::default() };
        Self { inner: Arc::new(inner) }
    }
}

impl<Eth> EthPubSub<Eth>
where
    Eth: EthSubscriptions,
{
    /// Returns the current sync status for the `syncing` subscription
    pub fn sync_status(&self, is_syncing: bool) -> PubSubSyncStatus {
        self.inner.sync_status(is_syncing)
    }

    /// Returns a stream that yields all transaction hashes emitted by the txpool.
    pub fn pending_transaction_hashes_stream(&self) -> impl Stream<Item = TxHash> {
        self.inner.pending_transaction_hashes_stream()
    }

    /// Returns a stream that yields all transactions emitted by the txpool.
    pub fn full_pending_transaction_stream(
        &self,
    ) -> impl Stream<Item = NewTransactionEvent<<Eth::Pool as TransactionPool>::Transaction>> {
        self.inner.full_pending_transaction_stream()
    }

    /// Returns a stream that yields new block headers.
    pub fn new_headers_stream(&self) -> impl Stream<Item = RpcHeader<Eth::NetworkTypes>> {
        self.inner.eth_api.header_stream()
    }

    /// Returns a stream that yields matching logs.
    pub fn log_stream(&self, filter: Filter) -> impl Stream<Item = RpcLog<Eth::NetworkTypes>> {
        self.inner.eth_api.log_stream(filter)
    }

    /// The actual handler for an accepted [`EthPubSub::subscribe`] call.
    pub async fn handle_accepted(
        &self,
        accepted_sink: SubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> Result<(), ErrorObject<'static>> {
        #[allow(unreachable_patterns)]
        match kind {
            SubscriptionKind::NewHeads => {
                pipe_from_stream(accepted_sink, self.new_headers_stream(), &self.inner.backlogs)
                    .await
            }
            SubscriptionKind::Logs => {
                // if no params are provided, used default filter params
                let filter = match params {
                    Some(Params::Logs(filter)) => *filter,
                    Some(Params::Bool(_)) => {
                        return Err(invalid_params_rpc_err("Invalid params for logs"))
                    }
                    _ => Default::default(),
                };
                pipe_from_stream(accepted_sink, self.log_stream(filter), &self.inner.backlogs).await
            }
            SubscriptionKind::NewPendingTransactions => {
                if let Some(params) = params {
                    match params {
                        Params::Bool(true) => {
                            // full transaction objects requested
                            let stream = self.full_pending_transaction_stream().filter_map(|tx| {
                                let tx_value = match self
                                    .inner
                                    .eth_api
                                    .converter()
                                    .fill_pending(tx.transaction.to_consensus())
                                {
                                    Ok(tx) => Some(tx),
                                    Err(err) => {
                                        error!(target = "rpc",
                                            %err,
                                            "Failed to fill transaction with block context"
                                        );
                                        None
                                    }
                                };
                                std::future::ready(tx_value)
                            });
                            return pipe_from_stream(accepted_sink, stream, &self.inner.backlogs)
                                .await
                        }
                        Params::Bool(false) | Params::None => {
                            // only hashes requested
                        }
                        _ => {
                            return Err(invalid_params_rpc_err(
                                "Invalid params for newPendingTransactions",
                            ))
                        }
                    }
                }

                pipe_from_stream(
                    accepted_sink,
                    self.pending_transaction_hashes_stream(),
                    &self.inner.backlogs,
                )
                .await
            }
            SubscriptionKind::Syncing => {
                // get new block subscription
                let mut canon_state = BroadcastStream::new(
                    self.inner.eth_api.provider().subscribe_to_canonical_state(),
                );
                // get current sync status
                let mut initial_sync_status = self.inner.eth_api.network().is_syncing();
                let current_sub_res = self.sync_status(initial_sync_status);

                // send the current status immediately
                let msg = SubscriptionMessage::new(
                    accepted_sink.method_name(),
                    accepted_sink.subscription_id(),
                    &current_sub_res,
                )
                .map_err(SubscriptionSerializeError::new)?;

                if accepted_sink.send(msg).await.is_err() {
                    return Ok(())
                }

                loop {
                    // Sends only happen when the sync status changes, so a failed send cannot be
                    // relied on to detect a closed subscription.
                    tokio::select! {
                        _ = accepted_sink.closed() => break,
                        maybe_event = canon_state.next() => {
                            if maybe_event.is_none() {
                                break
                            }
                        }
                    }

                    let current_syncing = self.inner.eth_api.network().is_syncing();
                    // Only send a new response if the sync status has changed
                    if current_syncing != initial_sync_status {
                        // Update the sync status on each new block
                        initial_sync_status = current_syncing;

                        // send a new message now that the status changed
                        let sync_status = self.sync_status(current_syncing);
                        let msg = SubscriptionMessage::new(
                            accepted_sink.method_name(),
                            accepted_sink.subscription_id(),
                            &sync_status,
                        )
                        .map_err(SubscriptionSerializeError::new)?;

                        if accepted_sink.send(msg).await.is_err() {
                            break
                        }
                    }
                }

                Ok(())
            }
            SubscriptionKind::TransactionReceipts => {
                let filter = match params {
                    Some(Params::TransactionReceipts(filter)) => filter,
                    None | Some(Params::None) => TransactionReceiptsParams::default(),
                    _ => {
                        return Err(invalid_params_rpc_err("Invalid params for transactionReceipts"))
                    }
                };

                pipe_from_stream(
                    accepted_sink,
                    self.inner.eth_api.transaction_receipts_stream(filter),
                    &self.inner.backlogs,
                )
                .await
            }
            _ => Err(invalid_params_rpc_err("Unsupported subscription kind")),
        }
    }
}

#[async_trait::async_trait]
impl<Eth> EthPubSubApiServer<RpcTransaction<Eth::NetworkTypes>> for EthPubSub<Eth>
where
    Eth: EthSubscriptions,
{
    /// Handler for `eth_subscribe`
    async fn subscribe(
        &self,
        pending: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let pubsub = self.clone();
        self.inner.subscription_task_spawner.spawn_task(async move {
            let _ = pubsub.handle_accepted(sink, kind, params).await;
        });

        Ok(())
    }
}

/// Helper to convert a serde error into an [`ErrorObject`]
#[derive(Debug, thiserror::Error)]
#[error("Failed to serialize subscription item: {0}")]
pub struct SubscriptionSerializeError(#[from] serde_json::Error);

impl SubscriptionSerializeError {
    const fn new(err: serde_json::Error) -> Self {
        Self(err)
    }
}

impl From<SubscriptionSerializeError> for ErrorObject<'static> {
    fn from(value: SubscriptionSerializeError) -> Self {
        internal_rpc_err(value.to_string())
    }
}

/// Pipes all stream items to the subscription sink.
///
/// Items are only queued while the connection's queued notifications stay within
/// [`MAX_QUEUED_NOTIFICATION_BYTES_PER_CONNECTION`], and the subscription is closed once its
/// connection has not drained for [`STALLED_SUBSCRIPTION_TIMEOUT`].
async fn pipe_from_stream<T, St>(
    mut sink: SubscriptionSink,
    stream: St,
    backlogs: &NotificationBacklogs,
) -> Result<(), ErrorObject<'static>>
where
    St: Stream<Item = T> + Unpin,
    T: Serialize,
{
    backlogs.register(sink.connection_id());
    let res = pipe_from_stream_within_budget(&mut sink, stream, backlogs).await;
    while !backlogs.unregister_if_drained(&sink) {
        tokio::time::sleep(BACKLOG_POLL_INTERVAL).await;
    }
    res
}

/// The body of [`pipe_from_stream`], run while the subscription is registered in `backlogs`.
async fn pipe_from_stream_within_budget<T, St>(
    sink: &mut SubscriptionSink,
    mut stream: St,
    backlogs: &NotificationBacklogs,
) -> Result<(), ErrorObject<'static>>
where
    St: Stream<Item = T> + Unpin,
    T: Serialize,
{
    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            },
            maybe_item = stream.next() => {
                let item = match maybe_item {
                    Some(item) => item,
                    None => {
                        // stream ended
                        break  Ok(())
                    },
                };
                let json =
                    serde_json::value::to_raw_value(&item).map_err(SubscriptionSerializeError::new)?;
                let len = json.get().len();
                let mut msg = SubscriptionMessage::from(json);
                let deadline = tokio::time::Instant::now() + STALLED_SUBSCRIPTION_TIMEOUT;

                loop {
                    match backlogs.try_send(sink, msg, len) {
                        Ok(()) => break,
                        Err(TrySendError::Full(unsent)) => msg = unsent,
                        Err(TrySendError::Closed(_)) => return Ok(()),
                    }

                    if tokio::time::Instant::now() >= deadline {
                        debug!(
                            target: "rpc::eth",
                            connection = sink.connection_id().0,
                            "Closing subscription of a connection that stopped reading"
                        );
                        return Ok(())
                    }

                    tokio::select! {
                        _ = sink.closed() => return Ok(()),
                        _ = tokio::time::sleep(BACKLOG_POLL_INTERVAL) => {}
                    }
                }
            }
        }
    }
}

impl<Eth> std::fmt::Debug for EthPubSub<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<EthApi> {
    /// The `eth` API.
    eth_api: EthApi,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Runtime,
    /// Notifications queued on each connection with active subscriptions.
    backlogs: Arc<NotificationBacklogs>,
}

// == impl EthPubSubInner ===

impl<Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Provider: BlockNumReader>,
{
    /// Returns the current sync status for the `syncing` subscription
    fn sync_status(&self, is_syncing: bool) -> PubSubSyncStatus {
        if is_syncing {
            let current_block = self
                .eth_api
                .provider()
                .chain_info()
                .map(|info| info.best_number)
                .unwrap_or_default();
            PubSubSyncStatus::Detailed(SyncStatusMetadata {
                syncing: true,
                starting_block: 0,
                current_block,
                highest_block: Some(current_block),
            })
        } else {
            PubSubSyncStatus::Simple(false)
        }
    }
}

impl<Eth> EthPubSubInner<Eth>
where
    Eth: RpcNodeCore<Pool: TransactionPool>,
{
    /// Returns a stream that yields all transaction hashes emitted by the txpool.
    fn pending_transaction_hashes_stream(&self) -> impl Stream<Item = TxHash> {
        ReceiverStream::new(self.eth_api.pool().pending_transactions_listener())
    }

    /// Returns a stream that yields all transactions emitted by the txpool.
    fn full_pending_transaction_stream(
        &self,
    ) -> impl Stream<Item = NewTransactionEvent<<Eth::Pool as TransactionPool>::Transaction>> {
        self.eth_api.pool().new_pending_pool_transactions_listener()
    }
}

/// Sizes of the notifications `eth_subscribe` recently queued on each connection.
#[derive(Debug, Default)]
struct NotificationBacklogs {
    connections: Mutex<HashMap<ConnectionId, NotificationBacklog>>,
}

impl NotificationBacklogs {
    /// Records a subscription that sends on `conn_id`.
    fn register(&self, conn_id: ConnectionId) {
        self.connections.lock().entry(conn_id).or_default().subscriptions += 1;
    }

    /// Removes a subscription [`Self::register`] recorded on `sink`'s connection, unless it is
    /// the connection's last one and notifications may still be queued on it.
    ///
    /// Keeping the last one until the queue drains, or the connection closes and drops it, stops a
    /// client from resetting its budget by subscribing again. Returns whether it was removed.
    fn unregister_if_drained(&self, sink: &SubscriptionSink) -> bool {
        let mut connections = self.connections.lock();
        let Entry::Occupied(mut entry) = connections.entry(sink.connection_id()) else {
            return true
        };
        let backlog = entry.get_mut();
        if backlog.subscriptions > 1 {
            backlog.subscriptions -= 1;
            return true
        }
        if backlog.retain_queued(sink.max_capacity() - sink.capacity()) > 0 {
            return false
        }
        entry.remove();
        true
    }

    /// Queues `msg`, `len` serialized bytes, on `sink` if the connection's queued notifications
    /// stay within [`MAX_QUEUED_NOTIFICATION_BYTES_PER_CONNECTION`] or its queue is empty.
    ///
    /// Returns [`TrySendError::Full`] with the message when it doesn't fit yet.
    fn try_send(
        &self,
        sink: &mut SubscriptionSink,
        msg: SubscriptionMessage,
        len: usize,
    ) -> Result<(), TrySendError> {
        let mut connections = self.connections.lock();
        let backlog = connections.entry(sink.connection_id()).or_default();
        let queued = backlog.retain_queued(sink.max_capacity() - sink.capacity());
        if queued > 0 && queued + len > MAX_QUEUED_NOTIFICATION_BYTES_PER_CONNECTION {
            return Err(TrySendError::Full(msg))
        }

        sink.try_send(msg)?;
        backlog.push(len);
        Ok(())
    }
}

/// Notifications queued on one connection.
#[derive(Debug, Default)]
struct NotificationBacklog {
    /// Subscriptions sending on the connection.
    subscriptions: usize,
    /// Serialized sizes of the most recently queued notifications, oldest first.
    sizes: VecDeque<usize>,
    /// Sum of `sizes`.
    total: usize,
}

impl NotificationBacklog {
    /// Forgets notifications that can no longer be queued, given `queued` messages in the
    /// connection's queue, and returns an upper bound on the queued notification bytes.
    ///
    /// The queue is FIFO and shared with ordinary responses, so any notification still in it is
    /// among the last `queued` ones sent. The message the connection is writing is not counted.
    fn retain_queued(&mut self, queued: usize) -> usize {
        while self.sizes.len() > queued {
            self.total -= self.sizes.pop_front().unwrap_or_default();
        }
        self.total
    }

    /// Records a notification of `len` serialized bytes that was queued.
    fn push(&mut self, len: usize) {
        self.sizes.push_back(len);
        self.total += len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpsee::{rpc_params, RpcModule};
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ITEM_LEN: usize = 1024 * 1024;
    const ITEMS: usize = 64;

    /// A module whose subscription pipes [`ITEMS`] strings of [`ITEM_LEN`] bytes through
    /// `backlogs`, counting the items it takes from the stream in `yielded`.
    fn large_items_module(
        yielded: Arc<AtomicUsize>,
        backlogs: Arc<NotificationBacklogs>,
    ) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        module
            .register_subscription("sub", "notif", "unsub", move |_, pending, _, _| {
                let yielded = yielded.clone();
                let backlogs = backlogs.clone();
                async move {
                    let sink = pending.accept().await.unwrap();
                    let stream = futures::stream::iter(0..ITEMS).map(move |_| {
                        yielded.fetch_add(1, Ordering::Relaxed);
                        "a".repeat(ITEM_LEN)
                    });
                    pipe_from_stream(sink, stream, &backlogs).await.unwrap();
                }
            })
            .unwrap();
        module
    }

    #[tokio::test]
    async fn unread_subscription_stops_at_byte_budget() {
        let yielded = Arc::new(AtomicUsize::new(0));
        let module = large_items_module(yielded.clone(), Arc::default());
        let _sub = module.subscribe("sub", rpc_params![], 1024).await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        // The notifications that fit in the budget are queued, and one more waits for room.
        let serialized_len = ITEM_LEN + 2;
        let expected = MAX_QUEUED_NOTIFICATION_BYTES_PER_CONNECTION / serialized_len + 1;
        assert_eq!(yielded.load(Ordering::Relaxed), expected);
    }

    #[tokio::test]
    async fn reading_subscription_receives_every_item() {
        let module = large_items_module(Arc::default(), Arc::default());
        let mut sub = module.subscribe("sub", rpc_params![], 1024).await.unwrap();

        for _ in 0..ITEMS {
            let (item, _) = sub.next::<String>().await.unwrap().unwrap();
            assert_eq!(item.len(), ITEM_LEN);
        }
    }

    #[tokio::test]
    async fn closed_connection_releases_backlog() {
        let backlogs = Arc::new(NotificationBacklogs::default());
        let module = large_items_module(Arc::default(), backlogs.clone());
        let sub = module.subscribe("sub", rpc_params![], 1024).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(backlogs.connections.lock().len(), 1);

        drop(sub);

        tokio::time::timeout(Duration::from_secs(5), async {
            while !backlogs.connections.lock().is_empty() {
                tokio::time::sleep(BACKLOG_POLL_INTERVAL).await;
            }
        })
        .await
        .unwrap();
    }
}
