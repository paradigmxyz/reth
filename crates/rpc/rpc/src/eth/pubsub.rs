//! `eth_` `PubSub` RPC handler implementation

use std::sync::Arc;

use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata, TransactionReceiptsParams,
    },
    Filter,
};
use futures::{FutureExt, StreamExt};
use jsonrpsee::{
    server::SubscriptionMessage, types::ErrorObject, PendingSubscriptionSink, SubscriptionSink,
};
use parking_lot::Mutex;
use reth_chain_state::CanonStateSubscriptions;
use reth_network_api::NetworkInfo;
use reth_rpc_convert::RpcHeader;
use reth_rpc_eth_api::{
    helpers::EthSubscriptions, pubsub::EthPubSubApiServer, EthApiTypes, RpcConvert, RpcLog,
    RpcNodeCore, RpcTransaction,
};
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_storage_api::BlockNumReader;
use reth_tasks::Runtime;
use reth_transaction_pool::{
    pool::NEW_TX_LISTENER_BUFFER_SIZE, NewTransactionEvent, TransactionPool,
};
use serde::Serialize;
use serde_json::value::RawValue;
use tokio::sync::{broadcast, OnceCell};
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};
use tracing::error;

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Eth: EthApiTypes> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Eth>>,
}

// === impl EthPubSub ===

impl<Eth: EthApiTypes> EthPubSub<Eth> {
    /// Creates a new, shareable instance.
    pub fn new(eth_api: Eth, subscription_task_spawner: Runtime) -> Self {
        let inner = EthPubSubInner {
            eth_api,
            subscription_task_spawner,
            all_logs: SharedFeed::new(LOG_FEED_CAPACITY),
            full_pending_txs: SharedFeed::new(PENDING_TX_FEED_CAPACITY),
        };
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

    /// Returns a stream that yields all transactions emitted by the txpool as RPC transactions.
    pub fn full_pending_rpc_transaction_stream(
        &self,
    ) -> impl Stream<Item = RpcTransaction<Eth::NetworkTypes>> {
        self.full_pending_transaction_stream().filter_map(|tx| {
            let tx_value =
                match self.inner.eth_api.converter().fill_pending(tx.transaction.to_consensus()) {
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
        })
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
                pipe_from_stream(accepted_sink, self.new_headers_stream()).await
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
                if filter == Filter::default() {
                    // Every unfiltered subscriber receives the same logs, so they share one feed
                    // that converts and encodes each log once.
                    let rx = self.inner.all_logs.subscribe(|tx| {
                        let pubsub = self.clone();
                        self.inner.subscription_task_spawner.spawn_task(async move {
                            forward(pubsub.log_stream(Filter::default()), tx).await
                        });
                    });
                    return pipe_shared(accepted_sink, rx).await
                }
                pipe_from_stream(accepted_sink, self.log_stream(filter)).await
            }
            SubscriptionKind::NewPendingTransactions => {
                if let Some(params) = params {
                    match params {
                        Params::Bool(true) => {
                            // Every subscriber requesting full transaction objects receives the
                            // same transactions, so they share one feed that converts and encodes
                            // each transaction once.
                            let rx = self.inner.full_pending_txs.subscribe(|tx| {
                                let pubsub = self.clone();
                                self.inner.subscription_task_spawner.spawn_task(async move {
                                    forward(pubsub.full_pending_rpc_transaction_stream(), tx).await
                                });
                            });
                            return pipe_shared(accepted_sink, rx).await
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

                pipe_from_stream(accepted_sink, self.pending_transaction_hashes_stream()).await
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
async fn pipe_from_stream<T, St>(
    sink: SubscriptionSink,
    mut stream: St,
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
                let msg = SubscriptionMessage::new(
                    sink.method_name(),
                    sink.subscription_id(),
                    &item
                ).map_err(SubscriptionSerializeError::new)?;

                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}

/// Pipes the items of shared batches to the subscription sink, reusing their cached JSON.
async fn pipe_shared<T: Serialize>(
    sink: SubscriptionSink,
    mut rx: broadcast::Receiver<Arc<SharedBatch<T>>>,
) -> Result<(), ErrorObject<'static>> {
    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            },
            res = rx.recv() => {
                let batch = match res {
                    Ok(batch) => batch,
                    // skip the batches missed while lagging behind
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        // feed ended
                        break Ok(())
                    }
                };
                for json in batch.json().await {
                    let msg = SubscriptionMessage::new(
                        sink.method_name(),
                        sink.subscription_id(),
                        json,
                    ).map_err(SubscriptionSerializeError::new)?;

                    if sink.send(msg).await.is_err() {
                        return Ok(())
                    }
                }
            }
        }
    }
}

impl<Eth: EthApiTypes> std::fmt::Debug for EthPubSub<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
struct EthPubSubInner<EthApi: EthApiTypes> {
    /// The `eth` API.
    eth_api: EthApi,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Runtime,
    /// Shared feed for `logs` subscriptions without a filter.
    all_logs: SharedFeed<RpcLog<EthApi::NetworkTypes>>,
    /// Shared feed for `newPendingTransactions` subscriptions with full transaction objects.
    full_pending_txs: SharedFeed<RpcTransaction<EthApi::NetworkTypes>>,
}

// == impl EthPubSubInner ===

impl<Eth> EthPubSubInner<Eth>
where
    Eth: EthApiTypes + RpcNodeCore<Provider: BlockNumReader>,
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
    Eth: EthApiTypes + RpcNodeCore<Pool: TransactionPool>,
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

/// Maximum number of ready items [`forward`] collects into one [`SharedBatch`].
const MAX_SHARED_BATCH_LEN: usize = 4096;

/// Capacity of the shared `logs` feed, in batches.
///
/// Its upstream produces about one batch per canonical state notification, so this matches the
/// buffer of the canonical state notification channel.
const LOG_FEED_CAPACITY: usize = 256;

/// Capacity of the shared `newPendingTransactions` feed, in batches.
///
/// Each batch holds at least one transaction, so a subscriber can fall at least as far behind as
/// the txpool listener feeding the upstream buffers by default.
const PENDING_TX_FEED_CAPACITY: usize = NEW_TX_LISTENER_BUFFER_SIZE;

/// Sender half of a [`SharedFeed`] channel.
type SharedSender<T> = broadcast::Sender<Arc<SharedBatch<T>>>;

/// An upstream shared by all subscribers of one subscription kind.
///
/// The upstream task starts with the first subscriber and exits once none are left, and the next
/// subscriber starts a new one.
struct SharedFeed<T> {
    /// The latest channel, whose only strong sender is owned by its upstream task.
    tx: Mutex<Option<broadcast::WeakSender<Arc<SharedBatch<T>>>>>,
    /// Capacity of each channel, in batches.
    capacity: usize,
}

impl<T> SharedFeed<T> {
    const fn new(capacity: usize) -> Self {
        Self { tx: Mutex::new(None), capacity }
    }

    /// Subscribes to the feed, calling `start` to spawn a new upstream task if no subscriber is
    /// live.
    ///
    /// `start` receives the only strong sender of the new channel, so subscribers see the channel
    /// close once the upstream task exits.
    fn subscribe(
        &self,
        start: impl FnOnce(SharedSender<T>),
    ) -> broadcast::Receiver<Arc<SharedBatch<T>>> {
        let mut weak_tx = self.tx.lock();
        // Receivers are only added here while at least one is live, so a channel that lost its
        // last receiver is never joined again and its upstream task can exit.
        if let Some(tx) = weak_tx.as_ref().and_then(|tx| tx.upgrade()) &&
            tx.receiver_count() > 0
        {
            return tx.subscribe()
        }
        let (tx, rx) = broadcast::channel(self.capacity);
        *weak_tx = Some(tx.downgrade());
        start(tx);
        rx
    }
}

/// Subscription items shared by all subscribers of a [`SharedFeed`], encoded to JSON at most once.
struct SharedBatch<T> {
    items: Vec<T>,
    /// Encoded by the first subscriber that sends the batch. tokio's [`OnceCell`] makes concurrent
    /// subscribers await that encoding instead of encoding again.
    json: OnceCell<Vec<Box<RawValue>>>,
}

impl<T: Serialize> SharedBatch<T> {
    fn new(items: Vec<T>) -> Arc<Self> {
        Arc::new(Self { items, json: OnceCell::new() })
    }

    /// Returns the JSON encoding of every item, encoding them on the first call.
    async fn json(&self) -> &[Box<RawValue>] {
        self.json
            .get_or_init(|| async {
                self.items
                    .iter()
                    .filter_map(|item| match serde_json::value::to_raw_value(item) {
                        Ok(json) => Some(json),
                        Err(err) => {
                            error!(target: "rpc::eth", %err, "Failed to serialize subscription item");
                            None
                        }
                    })
                    .collect()
            })
            .await
    }
}

/// Forwards `upstream` to `tx` until no subscriber is left, batching the items that are ready
/// together.
async fn forward<T, St>(upstream: St, tx: SharedSender<T>)
where
    St: Stream<Item = T> + Unpin,
    T: Serialize,
{
    let mut upstream = upstream.fuse();
    while let Some(item) = upstream.next().await {
        let mut items = vec![item];
        while items.len() < MAX_SHARED_BATCH_LEN &&
            let Some(Some(item)) = upstream.next().now_or_never()
        {
            items.push(item);
        }
        if tx.send(SharedBatch::new(items)).is_err() {
            break
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, Bytes, B256};
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::UnboundedReceiverStream;

    #[tokio::test]
    async fn shared_batch_json_matches_item_serialization() {
        let items = (0..3u8)
            .map(|i| alloy_rpc_types_eth::Log {
                inner: alloy_primitives::Log::new_unchecked(
                    Address::repeat_byte(i),
                    vec![B256::repeat_byte(i)],
                    Bytes::from(vec![i]),
                ),
                block_number: Some(i.into()),
                log_index: Some(i.into()),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let batch = SharedBatch::new(items.clone());

        let json = batch.json().await.iter().map(|json| json.get()).collect::<Vec<_>>();
        let expected =
            items.iter().map(|item| serde_json::to_string(item).unwrap()).collect::<Vec<_>>();
        assert_eq!(json, expected);
    }

    #[tokio::test]
    async fn forward_batches_ready_items_until_no_subscriber_is_left() {
        let (items_tx, items_rx) = mpsc::unbounded_channel();
        let (tx, mut rx) = broadcast::channel(1);
        items_tx.send(1u64).unwrap();
        items_tx.send(2).unwrap();
        let upstream = tokio::spawn(forward(UnboundedReceiverStream::new(items_rx), tx));

        assert_eq!(rx.recv().await.unwrap().items, [1, 2]);

        drop(rx);
        items_tx.send(3).unwrap();
        upstream.await.unwrap();
    }

    #[test]
    fn shared_feed_starts_one_upstream_per_live_channel() {
        let feed = SharedFeed::<u64>::new(1);
        let mut upstreams = Vec::new();

        let first = feed.subscribe(|tx| upstreams.push(tx));
        let second = feed.subscribe(|tx| upstreams.push(tx));
        assert_eq!(upstreams.len(), 1);

        // The channel lost its last receiver, so its upstream may already be exiting.
        drop((first, second));
        let _third = feed.subscribe(|tx| upstreams.push(tx));
        assert_eq!(upstreams.len(), 2);

        // Upstreams that ended drop their sender, which also requires a new one.
        upstreams.clear();
        let _fourth = feed.subscribe(|tx| upstreams.push(tx));
        assert_eq!(upstreams.len(), 1);
    }
}
