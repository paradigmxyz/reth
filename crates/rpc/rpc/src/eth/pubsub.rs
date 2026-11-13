//! `eth_` `PubSub` RPC handler implementation

use std::sync::Arc;

use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata, TransactionReceiptsParams,
    },
    Filter,
};
use futures::{stream::BoxStream, FutureExt, StreamExt};
use jsonrpsee::{
    server::{SharedResult, SubscriptionMessage},
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
use tokio::sync::broadcast::{self, error::RecvError};
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
                let results = self.subscribe_shared(
                    |feeds| &feeds.new_heads,
                    |pubsub| pubsub.new_headers_stream().boxed(),
                );
                pipe_from_shared(accepted_sink, results).await;
                Ok(())
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
                pipe_from_stream(accepted_sink, self.log_stream(filter)).await
            }
            SubscriptionKind::NewPendingTransactions => {
                if let Some(params) = params {
                    match params {
                        Params::Bool(true) => {
                            // full transaction objects requested
                            let results = self.subscribe_shared(
                                |feeds| &feeds.full_pending_transactions,
                                |pubsub| {
                                    let converter = pubsub.inner.eth_api.converter();
                                    pubsub
                                        .full_pending_transaction_stream()
                                        .filter_map(move |tx| {
                                            let tx_value = match converter
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
                                        })
                                        .boxed()
                                },
                            );
                            pipe_from_shared(accepted_sink, results).await;
                            return Ok(())
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

                let results = self.subscribe_shared(
                    |feeds| &feeds.pending_transaction_hashes,
                    |pubsub| pubsub.pending_transaction_hashes_stream().boxed(),
                );
                pipe_from_shared(accepted_sink, results).await;
                Ok(())
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
            _ => Err(invalid_params_rpc_err("Unsupported subscription kind")),
        }
    }

    /// Subscribes to the shared feed that `feed` selects, starting a producer over the stream that
    /// `items` returns if the feed has none.
    fn subscribe_shared<T: Serialize + 'static>(
        &self,
        feed: fn(&SharedFeeds) -> &SharedFeed,
        items: for<'a> fn(&'a Self) -> BoxStream<'a, T>,
    ) -> broadcast::Receiver<SharedResult> {
        let (receiver, sender) = feed(&self.inner.feeds).subscribe();
        if let Some(sender) = sender {
            let pubsub = self.clone();
            let mut producer =
                Box::pin(
                    async move { feed(&pubsub.inner.feeds).produce(sender, items(&pubsub)).await },
                );
            // The first poll subscribes the producer to its source, so it misses nothing that is
            // published before the spawned task first runs.
            if producer.as_mut().now_or_never().is_none() {
                self.inner.subscription_task_spawner.spawn_task(producer);
            }
        }
        receiver
    }
}

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

/// Sends the notifications of a shared feed to the subscription sink.
async fn pipe_from_shared(sink: SubscriptionSink, mut results: broadcast::Receiver<SharedResult>) {
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

    /// Subscribes to the feed. If no producer is running, also returns the sender that a new one
    /// must [produce](Self::produce) with.
    fn subscribe(
        &self,
    ) -> (broadcast::Receiver<SharedResult>, Option<broadcast::Sender<SharedResult>>) {
        let mut slot = self.sender.lock();
        if let Some(sender) = slot.as_ref().and_then(broadcast::WeakSender::upgrade) {
            return (sender.subscribe(), None)
        }
        let (sender, receiver) = broadcast::channel(self.capacity);
        *slot = Some(sender.downgrade());
        (receiver, Some(sender))
    }

    /// Serializes the items of `items` once and broadcasts them until the feed has no subscribers
    /// left or `items` ends.
    async fn produce<T: Serialize>(
        &self,
        sender: broadcast::Sender<SharedResult>,
        mut items: BoxStream<'_, T>,
    ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EthApiBuilder;
    use alloy_consensus::Transaction as _;
    use futures::stream;
    use jsonrpsee::RpcModule;
    use jsonrpsee_types::{SubscriptionId, SubscriptionPayload, SubscriptionResponse};
    use reth_chain_state::CanonStateNotification;
    use reth_chainspec::ChainSpecProvider;
    use reth_ethereum_primitives::Receipt;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_execution_types::{Chain, ExecutionOutcome};
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::SealedHeader;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
    };
    use reth_rpc_eth_api::EthApiTypes;
    use reth_testing_utils::generators::{self, BlockParams};
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TestPool},
        TransactionOrigin,
    };
    use serde_json::Value;
    use std::time::Duration;
    use tokio::sync::mpsc;

    type TestProvider = BlockchainProvider<MockNodeTypesWithDB>;

    /// How long a test waits for a notification or a feed to change.
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// Capacity of a test subscription's connection channel.
    const NOTIFICATION_BUFFER: usize = 16;

    fn test_provider() -> TestProvider {
        BlockchainProvider::with_latest(create_test_provider_factory(), SealedHeader::default())
            .unwrap()
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
                Receipt {
                    tx_type: tx.tx_type(),
                    success: true,
                    cumulative_gas_used,
                    logs: (0..2).map(|_| generators::random_log(&mut rng, None, Some(2))).collect(),
                }
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

    fn commit(provider: &TestProvider, chain: Chain) {
        provider
            .canonical_in_memory_state()
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
        let provider = test_provider();
        let pool = testing_pool();
        let pubsub = test_pubsub(provider.clone(), pool.clone());
        let module = pubsub.clone().into_rpc().remove_context();

        let (heads_id, mut heads) = subscribe(&module, r#"["newHeads"]"#).await;
        // Both ways of asking for every receipt share a feed.
        let mut receipts = Vec::new();
        for params in
            [r#"["transactionReceipts"]"#, r#"["transactionReceipts", {"transactionHashes": []}]"#]
        {
            receipts.push(subscribe(&module, params).await);
        }
        let (hashes_id, mut hashes) = subscribe(&module, r#"["newPendingTransactions"]"#).await;
        let (full_id, mut full) = subscribe(&module, r#"["newPendingTransactions", true]"#).await;
        let feeds = &pubsub.inner.feeds;
        wait_for_producers(&[
            (&feeds.new_heads, 1),
            (&feeds.transaction_receipts, 2),
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

        commit(&provider, chain_with_logs(1));

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

    #[tokio::test]
    async fn shared_feed_runs_only_while_subscribed() {
        let provider = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let feed = &pubsub.inner.feeds.new_heads;

        // Nothing runs before the first subscriber, even when blocks arrive.
        commit(&provider, chain_with_logs(1));
        tokio::task::yield_now().await;
        assert!(feed.sender.lock().is_none());

        // Two subscribers share one producer.
        let (first_id, mut first) = subscribe(&module, r#"["newHeads"]"#).await;
        let (second_id, mut second) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 2)]).await;
        let producer = feed.sender.lock().clone().unwrap();
        commit(&provider, chain_with_logs(2));
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
        commit(&provider, chain_with_logs(3));
        next_notification(&mut second).await;
        assert!(producer.upgrade().is_some());

        // It stops when the last one leaves.
        unsubscribe(&module, &second_id).await;
        wait_until_stopped(&producer).await;
        assert!(feed.sender.lock().is_none());

        // The next subscriber starts a new one.
        let (_, mut third) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 1)]).await;
        commit(&provider, chain_with_logs(4));
        next_notification(&mut third).await;
    }

    #[tokio::test]
    async fn lagging_subscriber_skips_oldest_notifications() {
        const BLOCKS: u64 = PER_BLOCK_FEED_CAPACITY as u64 + 64;

        let provider = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let (_, mut heads) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(&pubsub.inner.feeds.new_heads, 1)]).await;

        // The test doesn't read yet, so the subscriber blocks on its full connection channel
        // while the producer keeps broadcasting.
        for number in 1..=BLOCKS {
            commit(&provider, chain_with_logs(number));
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

    #[tokio::test]
    async fn new_producer_misses_nothing_published_before_it_runs() {
        let provider = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let mut results = pubsub.subscribe_shared(
            |feeds| &feeds.new_heads,
            |pubsub| pubsub.new_headers_stream().boxed(),
        );

        // Published before the spawned producer task first runs.
        commit(&provider, chain_with_logs(1));
        tokio::time::timeout(TIMEOUT, results.recv()).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unsubscribing_while_a_send_waits_stops_the_feed() {
        let provider = test_provider();
        let pubsub = test_pubsub(provider.clone(), testing_pool());
        let module = pubsub.clone().into_rpc().remove_context();
        let feed = &pubsub.inner.feeds.new_heads;
        let (id, _notifications) = subscribe(&module, r#"["newHeads"]"#).await;
        wait_for_producers(&[(feed, 1)]).await;
        let producer = feed.sender.lock().clone().unwrap();

        // The test doesn't read, so the subscription task ends up waiting for room to send.
        for number in 0..=NOTIFICATION_BUFFER as u64 {
            commit(&provider, chain_with_logs(number));
            tokio::task::yield_now().await;
        }
        unsubscribe(&module, &id).await;
        wait_until_stopped(&producer).await;
    }

    #[tokio::test]
    async fn producer_yields_while_its_source_stays_ready() {
        // More than the feed holds, all ready at once.
        const ITEMS: usize = 4 * PER_BLOCK_FEED_CAPACITY;

        let feed = Arc::new(SharedFeed::new(PER_BLOCK_FEED_CAPACITY));
        let (mut results, sender) = feed.subscribe();
        let sender = sender.unwrap();
        let producer = feed.clone();
        tokio::spawn(async move { producer.produce(sender, stream::iter(0..ITEMS).boxed()).await });

        // The subscriber gets to read during the burst, so it skips nothing.
        for _ in 0..ITEMS {
            tokio::time::timeout(TIMEOUT, results.recv()).await.unwrap().unwrap();
        }
        // The end of the source ends the feed.
        assert!(matches!(results.recv().await, Err(RecvError::Closed)));
        assert!(feed.sender.lock().is_none());
    }
}
