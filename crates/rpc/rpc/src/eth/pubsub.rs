//! `eth_` `PubSub` RPC handler implementation

use std::sync::Arc;

use alloy_primitives::TxHash;
use alloy_rpc_types_eth::{
    pubsub::{Params, PubSubSyncStatus, SubscriptionKind, SyncStatusMetadata},
    FilteredParams, Header, Log,
};
use futures::StreamExt;
use jsonrpsee::{
    server::SubscriptionMessage, types::ErrorObject, PendingSubscriptionSink, SubscriptionSink,
};
use reth_network_api::NetworkInfo;
use reth_primitives::NodePrimitives;
use reth_provider::{BlockNumReader, CanonStateSubscriptions};
use reth_rpc_eth_api::{
    pubsub::EthPubSubApiServer, EthApiTypes, RpcNodeCore, RpcTransaction, TransactionCompat,
};
use reth_rpc_eth_types::logs_utils;
use reth_rpc_server_types::result::{internal_rpc_err, invalid_params_rpc_err};
use reth_rpc_types_compat::transaction::from_recovered;
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::{NewTransactionEvent, PoolConsensusTx, TransactionPool};
use serde::Serialize;
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};
use tracing::error;

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Eth, Events> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Eth, Events>>,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Box<dyn TaskSpawner>,
}

// === impl EthPubSub ===

impl<Eth, Events> EthPubSub<Eth, Events> {
    /// Creates a new, shareable instance.
    ///
    /// Subscription tasks are spawned via [`tokio::task::spawn`]
    pub fn new(eth_api: Eth, chain_events: Events) -> Self {
        Self::with_spawner(eth_api, chain_events, Box::<TokioTaskExecutor>::default())
    }

    /// Creates a new, shareable instance.
    pub fn with_spawner(
        eth_api: Eth,
        chain_events: Events,
        subscription_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = EthPubSubInner { eth_api, chain_events };
        Self { inner: Arc::new(inner), subscription_task_spawner }
    }
}

#[async_trait::async_trait]
impl<Eth, Events> EthPubSubApiServer<RpcTransaction<Eth::NetworkTypes>> for EthPubSub<Eth, Events>
where
    Events: CanonStateSubscriptions + 'static,
    Eth: RpcNodeCore<Provider: BlockNumReader, Pool: TransactionPool, Network: NetworkInfo>
        + EthApiTypes<TransactionCompat: TransactionCompat<PoolConsensusTx<Eth::Pool>>>
        + 'static,
{
    /// Handler for `eth_subscribe`
    async fn subscribe(
        &self,
        pending: PendingSubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let pubsub = self.inner.clone();
        self.subscription_task_spawner.spawn(Box::pin(async move {
            let _ = handle_accepted(pubsub, sink, kind, params).await;
        }));

        Ok(())
    }
}

/// The actual handler for an accepted [`EthPubSub::subscribe`] call.
async fn handle_accepted<Eth, Events>(
    pubsub: Arc<EthPubSubInner<Eth, Events>>,
    accepted_sink: SubscriptionSink,
    kind: SubscriptionKind,
    params: Option<Params>,
) -> Result<(), ErrorObject<'static>>
where
    Events: CanonStateSubscriptions + 'static,
    Eth: RpcNodeCore<Provider: BlockNumReader, Pool: TransactionPool, Network: NetworkInfo>
        + EthApiTypes<TransactionCompat: TransactionCompat<PoolConsensusTx<Eth::Pool>>>,
{
    match kind {
        SubscriptionKind::NewHeads => {
            pipe_from_stream(accepted_sink, pubsub.new_headers_stream()).await
        }
        SubscriptionKind::Logs => {
            // if no params are provided, used default filter params
            let filter = match params {
                Some(Params::Logs(filter)) => FilteredParams::new(Some(*filter)),
                Some(Params::Bool(_)) => {
                    return Err(invalid_params_rpc_err("Invalid params for logs"))
                }
                _ => FilteredParams::default(),
            };
            pipe_from_stream(accepted_sink, pubsub.log_stream(filter)).await
        }
        SubscriptionKind::NewPendingTransactions => {
            if let Some(params) = params {
                match params {
                    Params::Bool(true) => {
                        // full transaction objects requested
                        let stream = pubsub.full_pending_transaction_stream().filter_map(|tx| {
                            let tx_value = match from_recovered(
                                tx.transaction.to_consensus(),
                                pubsub.eth_api.tx_resp_builder(),
                            ) {
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
                        return pipe_from_stream(accepted_sink, stream).await
                    }
                    Params::Bool(false) | Params::None => {
                        // only hashes requested
                    }
                    Params::Logs(_) => {
                        return Err(invalid_params_rpc_err(
                            "Invalid params for newPendingTransactions",
                        ))
                    }
                }
            }

            pipe_from_stream(accepted_sink, pubsub.pending_transaction_hashes_stream()).await
        }
        SubscriptionKind::Syncing => {
            // get new block subscription
            let mut canon_state =
                BroadcastStream::new(pubsub.chain_events.subscribe_to_canonical_state());
            // get current sync status
            let mut initial_sync_status = pubsub.eth_api.network().is_syncing();
            let current_sub_res = pubsub.sync_status(initial_sync_status);

            // send the current status immediately
            let msg = SubscriptionMessage::from_json(&current_sub_res)
                .map_err(SubscriptionSerializeError::new)?;
            if accepted_sink.send(msg).await.is_err() {
                return Ok(())
            }

            while canon_state.next().await.is_some() {
                let current_syncing = pubsub.eth_api.network().is_syncing();
                // Only send a new response if the sync status has changed
                if current_syncing != initial_sync_status {
                    // Update the sync status on each new block
                    initial_sync_status = current_syncing;

                    // send a new message now that the status changed
                    let sync_status = pubsub.sync_status(current_syncing);
                    let msg = SubscriptionMessage::from_json(&sync_status)
                        .map_err(SubscriptionSerializeError::new)?;
                    if accepted_sink.send(msg).await.is_err() {
                        break
                    }
                }
            }

            Ok(())
        }
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
                let msg = SubscriptionMessage::from_json(&item).map_err(SubscriptionSerializeError::new)?;
                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}

impl<Eth, Events> std::fmt::Debug for EthPubSub<Eth, Events> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<EthApi, Events> {
    /// The `eth` API.
    eth_api: EthApi,
    /// A type that allows to create new event subscriptions.
    chain_events: Events,
}

// == impl EthPubSubInner ===

impl<Eth, Events> EthPubSubInner<Eth, Events>
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

impl<Eth, Events> EthPubSubInner<Eth, Events>
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

impl<Eth, Events> EthPubSubInner<Eth, Events>
where
    Events: CanonStateSubscriptions,
{
    /// Returns a stream that yields all new RPC blocks.
    fn new_headers_stream(
        &self,
    ) -> impl Stream<Item = Header<<Events::Primitives as NodePrimitives>::BlockHeader>> {
        self.chain_events.canonical_state_stream().flat_map(|new_chain| {
            let headers = new_chain.committed().headers().collect::<Vec<_>>();
            futures::stream::iter(
                headers.into_iter().map(|h| Header::from_consensus(h.into(), None, None)),
            )
        })
    }

    /// Returns a stream that yields all logs that match the given filter.
    fn log_stream(&self, filter: FilteredParams) -> impl Stream<Item = Log> {
        BroadcastStream::new(self.chain_events.subscribe_to_canonical_state())
            .map(move |canon_state| {
                canon_state.expect("new block subscription never ends").block_receipts()
            })
            .flat_map(futures::stream::iter)
            .flat_map(move |(block_receipts, removed)| {
                let all_logs = logs_utils::matching_block_logs_with_tx_hashes(
                    &filter,
                    block_receipts.block,
                    block_receipts.tx_receipts.iter().map(|(tx, receipt)| (*tx, receipt)),
                    removed,
                );
                futures::stream::iter(all_logs)
            })
    }
}
