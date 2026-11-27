//! `eth_` `PubSub` RPC handler implementation

use std::{future::ready, sync::Arc};

use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata, TransactionReceiptsParams,
    },
    Filter,
};
use futures::{stream::BoxStream, FutureExt, StreamExt};
use jsonrpsee::{
    server::{PendingSubscriptionAcceptError, SharedResult, SubscriptionMessage},
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
use reth_transaction_pool::{
    pool::{NEW_TX_LISTENER_BUFFER_SIZE, PENDING_TX_LISTENER_BUFFER_SIZE},
    NewTransactionEvent, TransactionPool,
};
use serde::Serialize;
use tokio::sync::{
    broadcast::{self, error::RecvError},
    oneshot,
};
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};
use tracing::{debug, error};

/// Capacity of the shared feeds with one item per block. Like the canonical state notification
/// channel, it lets a subscriber fall 256 blocks behind before it skips the oldest.
const PER_BLOCK_FEED_CAPACITY: usize = 256;

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
        let inner =
            EthPubSubInner { eth_api, subscription_task_spawner, feeds: SharedFeeds::default() };
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

    /// Forwards notifications to an already accepted sink. Invalid parameters and per-subscriber
    /// serialization failures return an error. Shared feeds log failures and skip those items.
    pub async fn handle_accepted(
        &self,
        accepted_sink: SubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> Result<(), ErrorObject<'static>> {
        self.run_subscription(ready(Ok(accepted_sink)), Subscription::from_params(kind, params)?)
            .await
    }

    async fn run_subscription(
        &self,
        accepted_sink: impl Future<Output = Result<SubscriptionSink, PendingSubscriptionAcceptError>>,
        subscription: Subscription,
    ) -> Result<(), ErrorObject<'static>> {
        match subscription {
            Subscription::NewHeads => {
                let results = self.subscribe_shared(
                    |feeds| &feeds.new_heads,
                    |pubsub| pubsub.new_headers_stream().boxed(),
                );
                pipe_from_shared(accepted_sink, results).await;
                Ok(())
            }
            Subscription::Logs(filter) => {
                pipe_from_stream(accepted_sink, self.log_stream(*filter)).await
            }
            Subscription::FullPendingTransactions => {
                let results = self.subscribe_shared(
                    |feeds| &feeds.full_pending_transactions,
                    |pubsub| {
                        let converter = pubsub.inner.eth_api.converter();
                        pubsub
                            .full_pending_transaction_stream()
                            .filter_map(move |tx| {
                                let tx_value =
                                    match converter.fill_pending(tx.transaction.to_consensus()) {
                                        Ok(tx) => Some(tx),
                                        Err(err) => {
                                            error!(target = "rpc",
                                                %err,
                                                "Failed to fill transaction with block context"
                                            );
                                            None
                                        }
                                    };
                                ready(tx_value)
                            })
                            .boxed()
                    },
                );
                pipe_from_shared(accepted_sink, results).await;
                Ok(())
            }
            Subscription::PendingTransactionHashes => {
                let results = self.subscribe_shared(
                    |feeds| &feeds.pending_transaction_hashes,
                    |pubsub| pubsub.pending_transaction_hashes_stream().boxed(),
                );
                pipe_from_shared(accepted_sink, results).await;
                Ok(())
            }
            Subscription::Syncing => {
                // get new block subscription
                let mut canon_state = BroadcastStream::new(
                    self.inner.eth_api.provider().subscribe_to_canonical_state(),
                );
                let Ok(accepted_sink) = accepted_sink.await else { return Ok(()) };
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
            Subscription::TransactionReceipts(filter) => {
                if filter.transaction_hashes.as_ref().is_none_or(Vec::is_empty) {
                    // Without hashes every receipt matches, so all such subscribers get the same.
                    let results = self.subscribe_shared(
                        |feeds| &feeds.transaction_receipts,
                        |pubsub| {
                            pubsub
                                .inner
                                .eth_api
                                .transaction_receipts_stream(TransactionReceiptsParams::default())
                                .boxed()
                        },
                    );
                    pipe_from_shared(accepted_sink, results).await;
                    return Ok(())
                }

                pipe_from_stream(
                    accepted_sink,
                    self.inner.eth_api.transaction_receipts_stream(filter),
                )
                .await
            }
        }
    }

    /// Subscribes to the shared feed that `feed` selects, starting a producer over the stream that
    /// `items` returns if the feed has none.
    fn subscribe_shared<T: Serialize + Send + 'static>(
        &self,
        feed: fn(&SharedFeeds) -> &SharedFeed,
        items: for<'a> fn(&'a Self) -> BoxStream<'a, T>,
    ) -> broadcast::Receiver<SharedResult> {
        feed(&self.inner.feeds).subscribe(|sender| {
            let pubsub = self.clone();
            let mut producer = Box::pin(async move {
                let items = items(&pubsub);
                feed(&pubsub.inner.feeds).produce(sender, items).await;
            });
            let _guard = self.inner.subscription_task_spawner.handle().enter();
            if producer.as_mut().now_or_never().is_none() {
                self.inner.subscription_task_spawner.spawn_task(producer);
            }
        })
    }
}

impl<Eth> EthPubSubApiServer<RpcTransaction<Eth::NetworkTypes>> for EthPubSub<Eth>
where
    Eth: EthSubscriptions,
{
    /// Rejects invalid subscription parameters before returning a subscription ID.
    async fn subscribe(
        &self,
        pending: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult {
        let subscription = match Subscription::from_params(kind, params) {
            Ok(subscription) => subscription,
            Err(err) => {
                pending.reject(err).await;
                return Ok(())
            }
        };
        let (mut accepted, acknowledgement) = oneshot::channel();
        let pubsub = self.clone();
        self.inner.subscription_task_spawner.spawn_task(async move {
            // Canceling the forwarder must also cancel an acknowledgement waiting for room.
            let sink = async {
                let sink = tokio::select! {
                    biased;
                    _ = accepted.closed() => return Err(PendingSubscriptionAcceptError),
                    sink = pending.accept() => sink?,
                };
                accepted.send(()).map_err(|_| PendingSubscriptionAcceptError)?;
                Ok(sink)
            };
            if let Err(err) = pubsub.run_subscription(sink, subscription).await {
                error!(target: "rpc", %err, "Failed to forward subscription notification");
            }
        });
        acknowledgement.await.map_err(|_| PendingSubscriptionAcceptError.into())
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
    sink: impl Future<Output = Result<SubscriptionSink, PendingSubscriptionAcceptError>>,
    mut stream: St,
) -> Result<(), ErrorObject<'static>>
where
    St: Stream<Item = T> + Unpin,
    T: Serialize,
{
    let Ok(sink) = sink.await else { return Ok(()) };
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

/// Sends the notifications of a shared feed to the subscription sink.
async fn pipe_from_shared(
    sink: impl Future<Output = Result<SubscriptionSink, PendingSubscriptionAcceptError>>,
    mut results: broadcast::Receiver<SharedResult>,
) {
    let Ok(sink) = sink.await else { return };
    let forward = async {
        loop {
            match results.recv().await {
                Ok(result) => {
                    if sink.send(SubscriptionMessage::from_shared(result)).await.is_err() {
                        break
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    debug!(target: "rpc", skipped, "subscription lagging behind its shared feed")
                }
                // the feed ended
                Err(RecvError::Closed) => break,
            }
        }
    };
    // Also watched while a send waits for room, so an unsubscribed task drops its receiver and the
    // feed can stop.
    tokio::select! {
        _ = sink.closed() => {}
        _ = forward => {}
    }
}

impl<Eth> std::fmt::Debug for EthPubSub<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
struct EthPubSubInner<EthApi> {
    /// The `eth` API.
    eth_api: EthApi,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Runtime,
    /// Feeds of the subscriptions whose notifications are the same for every subscriber.
    feeds: SharedFeeds,
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

/// Feeds of the subscriptions whose notifications are the same for every subscriber.
struct SharedFeeds {
    /// `newHeads`.
    new_heads: SharedFeed,
    /// `newPendingTransactions` with full transaction objects.
    full_pending_transactions: SharedFeed,
    /// `newPendingTransactions` with transaction hashes.
    pending_transaction_hashes: SharedFeed,
    /// `transactionReceipts` without transaction hashes, which matches every receipt.
    transaction_receipts: SharedFeed,
}

impl Default for SharedFeeds {
    fn default() -> Self {
        Self {
            new_heads: SharedFeed::new(PER_BLOCK_FEED_CAPACITY),
            // As many transactions as a subscriber's own pool listener used to buffer.
            full_pending_transactions: SharedFeed::new(NEW_TX_LISTENER_BUFFER_SIZE),
            pending_transaction_hashes: SharedFeed::new(PENDING_TX_LISTENER_BUFFER_SIZE),
            transaction_receipts: SharedFeed::new(PER_BLOCK_FEED_CAPACITY),
        }
    }
}

/// Serializes the notifications of one subscription kind once and broadcasts them to all of its
/// subscribers.
///
/// The producer starts with the first subscriber and stops when the last one leaves, so nothing
/// is converted or serialized while the kind has no subscribers.
struct SharedFeed {
    /// Capacity of the broadcast channel.
    capacity: usize,
    /// The running producer's sender, if any.
    ///
    /// The producer holds the only strong sender, so if it dies its subscriptions end and the
    /// next subscriber starts a new one.
    sender: Mutex<Option<broadcast::WeakSender<SharedResult>>>,
}

impl SharedFeed {
    /// Creates a feed without a producer.
    const fn new(capacity: usize) -> Self {
        Self { capacity, sender: Mutex::new(None) }
    }

    /// Starts the producer under the feed lock so concurrent subscribers join after the
    /// upstream listener is attached.
    fn subscribe(
        &self,
        start: impl FnOnce(broadcast::Sender<SharedResult>),
    ) -> broadcast::Receiver<SharedResult> {
        let mut slot = self.sender.lock();
        if let Some(sender) = slot.as_ref().and_then(broadcast::WeakSender::upgrade) {
            return sender.subscribe()
        }
        let (sender, receiver) = broadcast::channel(self.capacity);
        *slot = Some(sender.downgrade());
        start(sender);
        receiver
    }

    /// Serializes the items of `items` once and broadcasts them until the feed has no subscribers
    /// left or `items` ends.
    async fn produce<T: Serialize>(
        &self,
        sender: broadcast::Sender<SharedResult>,
        mut items: BoxStream<'_, T>,
    ) {
        // The listener is attached. Poll its items in the spawned task, outside the feed lock.
        tokio::task::yield_now().await;
        loop {
            tokio::select! {
                _ = sender.closed() => {
                    if self.stop_if_unused(&sender) {
                        break
                    }
                }
                item = items.next() => {
                    let Some(item) = item else {
                        // Dropping the sender ends all subscriptions, like the end of their own
                        // streams did.
                        *self.sender.lock() = None;
                        break
                    };
                    match SharedResult::new(&item) {
                        Ok(result) => {
                            if sender.send(result).is_err() && self.stop_if_unused(&sender) {
                                break
                            }
                        }
                        Err(err) => error!(target: "rpc", %err, "Failed to serialize subscription item"),
                    }
                }
            }
            // A source that stays ready, like the blocks of one canonical update, never makes the
            // loop wait, so this lets the subscribers run before a whole burst is broadcast.
            tokio::task::consume_budget().await;
        }
        debug!(target: "rpc", "shared subscription feed stopped");
    }

    /// Removes the producer behind `sender` from the feed if it has no subscribers left, and
    /// returns whether it did.
    ///
    /// Subscribing holds the same lock, so a new subscriber either joins before this check or
    /// starts a new producer after it.
    fn stop_if_unused(&self, sender: &broadcast::Sender<SharedResult>) -> bool {
        let mut slot = self.sender.lock();
        if sender.receiver_count() > 0 {
            return false
        }
        *slot = None;
        true
    }
}

/// An `eth_subscribe` request with its parameters resolved for the requested kind.
enum Subscription {
    NewHeads,
    Logs(Box<Filter>),
    FullPendingTransactions,
    PendingTransactionHashes,
    Syncing,
    TransactionReceipts(TransactionReceiptsParams),
}

impl Subscription {
    fn from_params(
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> Result<Self, ErrorObject<'static>> {
        match (kind, params.unwrap_or_default()) {
            (SubscriptionKind::NewHeads, _) => Ok(Self::NewHeads),
            // Alloy decodes an object with `transactionHashes` as receipt params and drops its
            // `address` and `topics`, so it is rejected instead of being served as every log.
            (SubscriptionKind::Logs, Params::Logs(filter)) => Ok(Self::Logs(filter)),
            (SubscriptionKind::Logs, Params::None) => Ok(Self::Logs(Box::default())),
            (SubscriptionKind::NewPendingTransactions, Params::Bool(true)) => {
                Ok(Self::FullPendingTransactions)
            }
            (SubscriptionKind::NewPendingTransactions, Params::Bool(false) | Params::None) => {
                Ok(Self::PendingTransactionHashes)
            }
            (SubscriptionKind::Syncing, _) => Ok(Self::Syncing),
            (SubscriptionKind::TransactionReceipts, Params::TransactionReceipts(filter)) => {
                Ok(Self::TransactionReceipts(filter))
            }
            // Alloy decodes objects without transactionHashes as log filters. Like geth, ignore
            // log filter fields for receipt subscriptions.
            (SubscriptionKind::TransactionReceipts, Params::None | Params::Logs(_)) => {
                Ok(Self::TransactionReceipts(Default::default()))
            }
            _ => Err(invalid_params_rpc_err(format!("Invalid params for {kind}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EthApiBuilder;
    use alloy_consensus::Transaction as _;
    use futures::stream;
    use jsonrpsee::{
        server::{
            BoundedSubscriptions, ConnectionId, MethodCallback, MethodSink,
            RandomIntegerIdProvider, SubscriptionState,
        },
        RpcModule,
    };
    use jsonrpsee_types::{
        error::ErrorCode, Id, SubscriptionId, SubscriptionPayload, SubscriptionResponse,
    };
    use reth_chain_state::{CanonStateNotification, CanonicalInMemoryStateWriter};
    use reth_chainspec::{ChainSpecProvider, MAINNET};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Receipt;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_network_api::noop::NoopNetwork;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{
            create_test_provider_factory_with_in_memory_state_writer, MockNodeTypesWithDB,
        },
    };
    use reth_rpc_eth_api::EthApiTypes;
    use reth_testing_utils::generators::{self, BlockParams};
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TestPool},
        TransactionOrigin,
    };
    use serde_json::Value;
    use std::{future::pending, pin::pin, sync::mpsc::channel, thread, time::Duration};
    use tokio::{
        runtime::{Builder, Handle},
        sync::mpsc,
    };

    type TestProvider = BlockchainProvider<MockNodeTypesWithDB>;

    /// How long a test waits for a notification or a feed to change.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// Capacity of a test subscription's connection channel.
    const NOTIFICATION_BUFFER: usize = 16;

    fn test_provider() -> (TestProvider, CanonicalInMemoryStateWriter) {
        let (factory, in_memory_state_writer) =
            create_test_provider_factory_with_in_memory_state_writer(MAINNET.clone());
        init_genesis(&factory).unwrap();
        let canonical_in_memory_state_writer = CanonicalInMemoryStateWriter::new(
            factory.chain_info_headers().unwrap(),
            in_memory_state_writer,
        );
        let provider = BlockchainProvider::with_canonical_in_memory_state(
            factory,
            canonical_in_memory_state_writer.canonical_in_memory_state().clone(),
        );
        (provider, canonical_in_memory_state_writer)
    }

    fn test_pubsub(provider: TestProvider, pool: TestPool) -> EthPubSub<impl EthSubscriptions> {
        let evm_config = EthEvmConfig::new(provider.chain_spec());
        let eth_api =
            EthApiBuilder::new(provider, pool, NoopNetwork::default(), evm_config).build();
        EthPubSub::new(eth_api, Runtime::test())
    }

    /// Returns a chain of one block with transactions whose receipts have logs.
    fn chain_with_logs(number: u64) -> Chain {
        let mut rng = generators::rng();
        let block = generators::random_block(
            &mut rng,
            number,
            BlockParams { tx_count: Some(3), ommers_count: Some(0), ..Default::default() },
        );
        let mut cumulative_gas_used = 0;
        let receipts = block
            .body()
            .transactions
            .iter()
            .map(|tx| {
                cumulative_gas_used += tx.gas_limit();
                Receipt::standard(
                    tx.tx_type(),
                    true,
                    cumulative_gas_used,
                    (0..2).map(|_| generators::random_log(&mut rng, None, Some(2))).collect(),
                )
            })
            .collect();
        Chain::new(
            [block.try_recover().unwrap()],
            ExecutionOutcome {
                receipts: vec![receipts],
                first_block: number,
                ..Default::default()
            },
            Default::default(),
        )
    }

    fn commit(canonical_in_memory_state_writer: &mut CanonicalInMemoryStateWriter, chain: Chain) {
        canonical_in_memory_state_writer
            .notify_canon_state(CanonStateNotification::Commit { new: Arc::new(chain) });
    }

    /// Subscribes with `params` and returns the subscription id and its notifications.
    async fn subscribe(
        module: &RpcModule<()>,
        params: &str,
    ) -> (SubscriptionId<'static>, mpsc::Receiver<String>) {
        let request =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":{params}}}"#);
        let (response, notifications) =
            module.raw_json_request(&request, NOTIFICATION_BUFFER).await.unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        let id = response["result"].as_u64().unwrap_or_else(|| panic!("{response}"));
        (SubscriptionId::Num(id), notifications)
    }

    async fn unsubscribe(module: &RpcModule<()>, id: &SubscriptionId<'static>) {
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"eth_unsubscribe","params":[{}]}}"#,
            serde_json::to_string(id).unwrap()
        );
        let (response, _) = module.raw_json_request(&request, 1).await.unwrap();
        assert_eq!(response, r#"{"jsonrpc":"2.0","id":2,"result":true}"#);
    }

    async fn next_notification(notifications: &mut mpsc::Receiver<String>) -> String {
        tokio::time::timeout(TIMEOUT, notifications.recv()).await.unwrap().unwrap()
    }

    /// The notification as subscription tasks built it before results were shared: serialized
    /// per subscriber by `SubscriptionMessage::new`.
    fn serialized_notification(id: &SubscriptionId<'static>, result: &impl Serialize) -> String {
        serde_json::to_string(&SubscriptionResponse::new(
            "eth_subscription".into(),
            SubscriptionPayload { subscription: id.clone(), result },
        ))
        .unwrap()
    }

    /// Returns how many subscribers the feed's running producer has, or `None` without one.
    fn feed_subscribers(feed: &SharedFeed) -> Option<usize> {
        feed.sender.lock().as_ref()?.upgrade().map(|sender| sender.receiver_count())
    }

    /// Waits until each feed has a running producer with the given number of subscribers.
    async fn wait_for_producers(feeds: &[(&SharedFeed, usize)]) {
        tokio::time::timeout(TIMEOUT, async {
            while !feeds.iter().all(|(feed, count)| feed_subscribers(feed) == Some(*count)) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    /// Waits until the producer behind `sender` has exited.
    async fn wait_until_stopped(sender: &broadcast::WeakSender<SharedResult>) {
        tokio::time::timeout(TIMEOUT, async {
            while sender.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn shared_notifications_match_serialized_notifications() {
        let (provider, mut canonical_in_memory_state_writer) = test_provider();
        let pool = testing_pool();
        let pubsub = test_pubsub(provider.clone(), pool.clone());
        let module = pubsub.clone().into_rpc().remove_context();

        let (heads_id, mut heads) = subscribe(&module, r#"["newHeads"]"#).await;
        // Unfiltered receipt subscriptions share a feed.
        let mut receipts = Vec::new();
        for params in [
            r#"["transactionReceipts"]"#,
            r#"["transactionReceipts", {}]"#,
            r#"["transactionReceipts", {"address": "0x0000000000000000000000000000000000000001"}]"#,
            r#"["transactionReceipts", {"transactionHashes": null}]"#,
            r#"["transactionReceipts", {"transactionHashes": []}]"#,
        ] {
            receipts.push(subscribe(&module, params).await);
        }
        let (hashes_id, mut hashes) = subscribe(&module, r#"["newPendingTransactions"]"#).await;
        let (full_id, mut full) = subscribe(&module, r#"["newPendingTransactions", true]"#).await;
        let feeds = &pubsub.inner.feeds;
        wait_for_producers(&[
            (&feeds.new_heads, 1),
            (&feeds.transaction_receipts, receipts.len()),
            (&feeds.pending_transaction_hashes, 1),
            (&feeds.full_pending_transactions, 1),
        ])
        .await;

        // The items subscription tasks used to serialize, from the same sources.
        let mut expected_heads = pubsub.new_headers_stream().boxed();
        let mut expected_receipts =
            pubsub.inner.eth_api.transaction_receipts_stream(Default::default()).boxed();
        let mut expected_hashes = pubsub.pending_transaction_hashes_stream().boxed();
        let mut expected_full = pubsub.full_pending_transaction_stream().boxed();

        commit(&mut canonical_in_memory_state_writer, chain_with_logs(1));

        let header = expected_heads.next().await.unwrap();
        assert_eq!(
            next_notification(&mut heads).await,
            serialized_notification(&heads_id, &header)
        );

        let block_receipts = expected_receipts.next().await.unwrap();
        for (id, notifications) in &mut receipts {
            assert_eq!(
                next_notification(notifications).await,
                serialized_notification(id, &block_receipts)
            );
        }

        pool.add_transaction(TransactionOrigin::External, MockTransaction::eip1559())
            .await
            .unwrap();

        let hash = expected_hashes.next().await.unwrap();
        assert_eq!(
            next_notification(&mut hashes).await,
            serialized_notification(&hashes_id, &hash)
        );

        let tx = expected_full.next().await.unwrap();
        let tx =
            pubsub.inner.eth_api.converter().fill_pending(tx.transaction.to_consensus()).unwrap();
        assert_eq!(next_notification(&mut full).await, serialized_notification(&full_id, &tx));

        // Nothing else was sent.
        let receipts = receipts.iter_mut().map(|(_, notifications)| notifications);
        for notifications in receipts.chain([&mut heads, &mut hashes, &mut full]) {
            assert!(notifications.try_recv().is_err());
        }
    }

    #[test]
    fn receipt_subscription_preserves_transaction_hashes() {
        let hash = TxHash::repeat_byte(1);
        let params =
            serde_json::from_value(serde_json::json!({"transactionHashes": [hash], "foo": 1}))
                .unwrap();
        let Subscription::TransactionReceipts(filter) =
            Subscription::from_params(SubscriptionKind::TransactionReceipts, Some(params)).unwrap()
        else {
            panic!("expected a transaction receipts subscription");
        };
        assert_eq!(filter.transaction_hashes, Some(vec![hash]));
    }

    #[tokio::test]
    async fn shared_feed_runs_only_while_subscribed() {
        let (provider, mut canonical_in_memory_state_writer) = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let feed = &pubsub.inner.feeds.new_heads;

        // Nothing runs before the first subscriber, even when blocks arrive.
        commit(&mut canonical_in_memory_state_writer, chain_with_logs(1));
        tokio::task::yield_now().await;
        assert!(feed.sender.lock().is_none());

        // Two subscribers share one producer.
        let (first_id, mut first) = subscribe(&module, r#"["newHeads"]"#).await;
        let (second_id, mut second) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 2)]).await;
        let producer = feed.sender.lock().clone().unwrap();
        commit(&mut canonical_in_memory_state_writer, chain_with_logs(2));
        let notification = next_notification(&mut first).await;
        assert_eq!(
            next_notification(&mut second).await,
            notification.replace(
                &serde_json::to_string(&first_id).unwrap(),
                &serde_json::to_string(&second_id).unwrap()
            )
        );

        // It keeps running while one subscriber is left.
        unsubscribe(&module, &first_id).await;
        wait_for_producers(&[(feed, 1)]).await;
        commit(&mut canonical_in_memory_state_writer, chain_with_logs(3));
        next_notification(&mut second).await;
        assert!(producer.upgrade().is_some());

        // It stops when the last one leaves.
        unsubscribe(&module, &second_id).await;
        wait_until_stopped(&producer).await;
        assert!(feed.sender.lock().is_none());

        // The next subscriber starts a new one.
        let (_, mut third) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 1)]).await;
        commit(&mut canonical_in_memory_state_writer, chain_with_logs(4));
        next_notification(&mut third).await;
    }

    #[tokio::test]
    async fn lagging_subscriber_skips_oldest_notifications() {
        const BLOCKS: u64 = PER_BLOCK_FEED_CAPACITY as u64 + 64;

        let (provider, mut canonical_in_memory_state_writer) = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let (_, mut heads) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(&pubsub.inner.feeds.new_heads, 1)]).await;

        // The test doesn't read yet, so the subscriber blocks on its full connection channel
        // while the producer keeps broadcasting.
        for number in 1..=BLOCKS {
            commit(&mut canonical_in_memory_state_writer, chain_with_logs(number));
            tokio::task::yield_now().await;
        }

        let mut received = Vec::new();
        while received.last() != Some(&BLOCKS) {
            let notification: Value =
                serde_json::from_str(&next_notification(&mut heads).await).unwrap();
            let header: alloy_rpc_types_eth::Header =
                serde_json::from_value(notification["params"]["result"].clone()).unwrap();
            received.push(header.number);
        }
        assert!(received.len() < BLOCKS as usize, "nothing was skipped: {received:?}");
        assert!(received.is_sorted_by(|a, b| a < b), "{received:?}");
    }

    #[test]
    fn subscriptions_receive_the_first_block_after_acknowledgement() {
        let rpc = Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        let forward = Builder::new_current_thread().enable_all().build().unwrap();
        let (pubsub, mut writer) = forward.block_on(async {
            let (provider, writer) = test_provider();
            (test_pubsub(provider, testing_pool()), writer)
        });
        let module = pubsub.clone().into_rpc().remove_context();
        let chain = chain_with_logs(1);
        let hash = *chain.transaction_hashes().nth(1).unwrap();
        let params =
            serde_json::json!(["transactionReceipts", { "transactionHashes": [hash] }]).to_string();
        let mut request = pin!(subscribe(&module, &params));
        // The paused RPC clock advances to the timeout once its tasks are idle.
        rpc.block_on(async {
            assert!(
                tokio::time::timeout(TIMEOUT, &mut request).await.is_err(),
                "acknowledged before the source could attach"
            );
        });
        let (_, mut notifications) =
            forward.block_on(async { tokio::time::timeout(TIMEOUT, request).await.unwrap() });

        let mut results = {
            let _guard = rpc.enter();
            pubsub.subscribe_shared(
                |feeds| &feeds.new_heads,
                |pubsub| {
                    assert_eq!(
                        Handle::current().id(),
                        pubsub.inner.subscription_task_spawner.handle().id()
                    );
                    pubsub.new_headers_stream().boxed()
                },
            )
        };

        // Commit once after the receipt acknowledgement, before the new header producer runs.
        commit(&mut writer, chain);
        forward.block_on(async {
            tokio::time::timeout(TIMEOUT, results.recv()).await.unwrap().unwrap();
            let msg = next_notification(&mut notifications).await;
            let notification: Value = serde_json::from_str(&msg).unwrap();
            let receipts = notification["params"]["result"].as_array().unwrap();
            assert_eq!(receipts.len(), 1);
            assert_eq!(receipts[0]["transactionHash"], serde_json::json!(hash));
        });
    }

    #[tokio::test]
    async fn unsubscribing_while_a_send_waits_stops_the_feed() {
        let (provider, mut canonical_in_memory_state_writer) = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let feed = &pubsub.inner.feeds.new_heads;
        let (id, _notifications) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 1)]).await;
        let producer = feed.sender.lock().clone().unwrap();

        // The test doesn't read, so the subscription task ends up waiting for room to send.
        for number in 0..=NOTIFICATION_BUFFER as u64 {
            commit(&mut canonical_in_memory_state_writer, chain_with_logs(number));
            tokio::task::yield_now().await;
        }
        unsubscribe(&module, &id).await;
        wait_until_stopped(&producer).await;
    }

    #[tokio::test]
    async fn producer_yields_while_its_source_stays_ready() {
        // More than the feed holds, all ready at once.
        const ITEMS: usize = 4 * PER_BLOCK_FEED_CAPACITY;

        let pubsub = test_pubsub(test_provider().0, testing_pool());
        let mut results =
            pubsub.subscribe_shared(|feeds| &feeds.new_heads, |_| stream::iter(0..ITEMS).boxed());

        // The subscriber gets to read during the burst, so it skips nothing.
        for _ in 0..ITEMS {
            tokio::time::timeout(TIMEOUT, results.recv()).await.unwrap().unwrap();
        }
        // The end of the source ends the feed.
        assert!(matches!(results.recv().await, Err(RecvError::Closed)));
        assert!(pubsub.inner.feeds.new_heads.sender.lock().is_none());
    }

    #[test]
    fn joining_subscriber_waits_for_source_startup() {
        let feed = &SharedFeed::new(1);
        let (started, starting) = channel();
        let (resume, paused) = channel();
        thread::scope(|scope| {
            let first = scope.spawn(move || {
                feed.subscribe(|sender| {
                    started.send(sender).unwrap();
                    paused.recv_timeout(TIMEOUT).unwrap();
                })
            });
            let producer = starting.recv_timeout(TIMEOUT).unwrap();
            assert!(feed.sender.try_lock().is_none(), "feed exposed during source startup");
            resume.send(()).unwrap();
            let second = feed.subscribe(|_| panic!("expected to share the first producer"));
            let first = first.join().unwrap();
            assert!(first.same_channel(&second));
            assert_eq!(producer.receiver_count(), 2);
        });
    }

    #[test]
    fn direct_subscription_waits_for_acknowledgement_before_polling() {
        let stream = stream::iter([42]).inspect(|_| panic!("polled before acknowledgement"));
        assert!(pipe_from_stream(pending(), stream).now_or_never().is_none());
    }

    #[tokio::test]
    async fn canceled_subscription_startup_answers_in_process_request() {
        let pubsub = test_pubsub(test_provider().0, testing_pool());
        let runtime = &pubsub.inner.subscription_task_spawner;
        runtime.graceful_shutdown();
        tokio::time::timeout(TIMEOUT, runtime.on_shutdown_signal().clone()).await.unwrap();

        let module = pubsub.into_rpc();
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"eth_subscribe","params":["newHeads"]}"#;
        let (response, _) = tokio::time::timeout(TIMEOUT, module.raw_json_request(request, 1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response,
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Internal error"}}"#
        );
    }

    #[tokio::test]
    async fn canceled_subscriptions_cannot_send_blocked_ack() {
        let pubsub = test_pubsub(test_provider().0, testing_pool());
        let module = pubsub.clone().into_rpc();
        let Some(MethodCallback::Subscription(subscribe)) = module.method("eth_subscribe") else {
            panic!("expected a subscription")
        };
        let feed = &pubsub.inner.feeds.new_heads;
        let start = async |id| {
            let (tx, messages) = mpsc::channel(1);
            let full = tx.clone().try_reserve_owned().unwrap();
            let response = subscribe(
                Id::Number(id),
                jsonrpsee_types::Params::new(Some(r#"["newHeads"]"#)),
                MethodSink::new(tx),
                SubscriptionState {
                    conn_id: ConnectionId::IN_PROCESS,
                    id_provider: &RandomIntegerIdProvider,
                    subscription_permit: BoundedSubscriptions::new(1).acquire().unwrap(),
                },
            );
            wait_for_producers(&[(feed, 1)]).await;
            (response, feed.sender.lock().clone().unwrap(), full, messages)
        };

        let (canceled, producer, full, mut messages) = start(1).await;
        drop(canceled);
        wait_until_stopped(&producer).await;
        drop(full);
        assert_eq!(tokio::time::timeout(TIMEOUT, messages.recv()).await.unwrap(), None);

        let (active, producer, full, mut messages) = start(2).await;
        pubsub.inner.subscription_task_spawner.graceful_shutdown();
        wait_until_stopped(&producer).await;
        drop(full);

        let response = tokio::time::timeout(TIMEOUT, active).await.unwrap();
        assert_eq!(response.as_error_code(), Some(ErrorCode::InternalError.code()));
        assert_eq!(tokio::time::timeout(TIMEOUT, messages.recv()).await.unwrap(), None);
    }
}
