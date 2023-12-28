//! `eth_` PubSub RPC handler implementation

use crate::{eth::logs_utils, result::invalid_params_rpc_err};
use futures::StreamExt;
use jsonrpsee::{server::SubscriptionMessage, PendingSubscriptionSink, SubscriptionSink};
use reth_network_api::NetworkInfo;
use reth_primitives::{IntoRecoveredTransaction, TxHash};
use reth_provider::{BlockReader, CanonStateSubscriptions, EvmEnvProvider};
use reth_rpc_api::EthPubSubApiServer;
use reth_rpc_types::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SubscriptionResult as EthSubscriptionResult,
        SyncStatusMetadata,
    },
    FilteredParams, Header, Log,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::{NewTransactionEvent, TransactionPool};
use serde::Serialize;
use std::sync::Arc;
use tokio_stream::{
    wrappers::{BroadcastStream, ReceiverStream},
    Stream,
};

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Provider, Pool, Events, Network> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Provider, Pool, Events, Network>>,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Box<dyn TaskSpawner>,
}

// === impl EthPubSub ===

impl<Provider, Pool, Events, Network> EthPubSub<Provider, Pool, Events, Network> {
    /// Creates a new, shareable instance.
    ///
    /// Subscription tasks are spawned via [tokio::task::spawn]
    pub fn new(provider: Provider, pool: Pool, chain_events: Events, network: Network) -> Self {
        Self::with_spawner(
            provider,
            pool,
            chain_events,
            network,
            Box::<TokioTaskExecutor>::default(),
        )
    }

    /// Creates a new, shareable instance.
    pub fn with_spawner(
        provider: Provider,
        pool: Pool,
        chain_events: Events,
        network: Network,
        subscription_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = EthPubSubInner { provider, pool, chain_events, network };
        Self { inner: Arc::new(inner), subscription_task_spawner }
    }
}

#[async_trait::async_trait]
impl<Provider, Pool, Events, Network> EthPubSubApiServer
    for EthPubSub<Provider, Pool, Events, Network>
where
    Provider: BlockReader + EvmEnvProvider + Clone + 'static,
    Pool: TransactionPool + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    Network: NetworkInfo + Clone + 'static,
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
async fn handle_accepted<Provider, Pool, Events, Network>(
    pubsub: Arc<EthPubSubInner<Provider, Pool, Events, Network>>,
    accepted_sink: SubscriptionSink,
    kind: SubscriptionKind,
    params: Option<Params>,
) -> Result<(), jsonrpsee::core::Error>
where
    Provider: BlockReader + EvmEnvProvider + Clone + 'static,
    Pool: TransactionPool + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    Network: NetworkInfo + Clone + 'static,
{
    match kind {
        SubscriptionKind::NewHeads => {
            let stream = pubsub
                .new_headers_stream()
                .map(|block| EthSubscriptionResult::Header(Box::new(block.into())));
            pipe_from_stream(accepted_sink, stream).await
        }
        SubscriptionKind::Logs => {
            // if no params are provided, used default filter params
            let filter = match params {
                Some(Params::Logs(filter)) => FilteredParams::new(Some(*filter)),
                Some(Params::Bool(_)) => {
                    return Err(invalid_params_rpc_err("Invalid params for logs").into())
                }
                _ => FilteredParams::default(),
            };
            let stream =
                pubsub.log_stream(filter).map(|log| EthSubscriptionResult::Log(Box::new(log)));
            pipe_from_stream(accepted_sink, stream).await
        }
        SubscriptionKind::NewPendingTransactions => {
            if let Some(params) = params {
                match params {
                    Params::Bool(true) => {
                        // full transaction objects requested
                        let stream = pubsub.full_pending_transaction_stream().map(|tx| {
                            EthSubscriptionResult::FullTransaction(Box::new(
                                reth_rpc_types_compat::transaction::from_recovered(
                                    tx.transaction.to_recovered_transaction(),
                                ),
                            ))
                        });
                        return pipe_from_stream(accepted_sink, stream).await
                    }
                    Params::Bool(false) | Params::None => {
                        // only hashes requested
                    }
                    Params::Logs(_) => {
                        return Err(invalid_params_rpc_err(
                            "Invalid params for newPendingTransactions",
                        )
                        .into())
                    }
                }
            }

            let stream = pubsub
                .pending_transaction_hashes_stream()
                .map(EthSubscriptionResult::TransactionHash);
            pipe_from_stream(accepted_sink, stream).await
        }
        SubscriptionKind::Syncing => {
            // get new block subscription
            let mut canon_state =
                BroadcastStream::new(pubsub.chain_events.subscribe_to_canonical_state());
            // get current sync status
            let mut initial_sync_status = pubsub.network.is_syncing();
            let current_sub_res = pubsub.sync_status(initial_sync_status).await;

            // send the current status immediately
            let msg = SubscriptionMessage::from_json(&current_sub_res)?;
            if accepted_sink.send(msg).await.is_err() {
                return Ok(())
            }

            while (canon_state.next().await).is_some() {
                let current_syncing = pubsub.network.is_syncing();
                // Only send a new response if the sync status has changed
                if current_syncing != initial_sync_status {
                    // Update the sync status on each new block
                    initial_sync_status = current_syncing;

                    // send a new message now that the status changed
                    let sync_status = pubsub.sync_status(current_syncing).await;
                    let msg = SubscriptionMessage::from_json(&sync_status)?;
                    if accepted_sink.send(msg).await.is_err() {
                        break
                    }
                }
            }

            Ok(())
        }
    }
}

/// Pipes all stream items to the subscription sink.
async fn pipe_from_stream<T, St>(
    sink: SubscriptionSink,
    mut stream: St,
) -> Result<(), jsonrpsee::core::Error>
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
                let msg = SubscriptionMessage::from_json(&item)?;
                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}

impl<Provider, Pool, Events, Network> std::fmt::Debug
    for EthPubSub<Provider, Pool, Events, Network>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<Provider, Pool, Events, Network> {
    /// The transaction pool.
    pool: Pool,
    /// The provider that can interact with the chain.
    provider: Provider,
    /// A type that allows to create new event subscriptions.
    chain_events: Events,
    /// The network.
    network: Network,
}

// == impl EthPubSubInner ===

impl<Provider, Pool, Events, Network> EthPubSubInner<Provider, Pool, Events, Network>
where
    Provider: BlockReader + 'static,
{
    /// Returns the current sync status for the `syncing` subscription
    async fn sync_status(&self, is_syncing: bool) -> EthSubscriptionResult {
        if is_syncing {
            let current_block =
                self.provider.chain_info().map(|info| info.best_number).unwrap_or_default();
            EthSubscriptionResult::SyncState(PubSubSyncStatus::Detailed(SyncStatusMetadata {
                syncing: true,
                starting_block: 0,
                current_block,
                highest_block: Some(current_block),
            }))
        } else {
            EthSubscriptionResult::SyncState(PubSubSyncStatus::Simple(false))
        }
    }
}

impl<Provider, Pool, Events, Network> EthPubSubInner<Provider, Pool, Events, Network>
where
    Pool: TransactionPool + 'static,
{
    /// Returns a stream that yields all transaction hashes emitted by the txpool.
    fn pending_transaction_hashes_stream(&self) -> impl Stream<Item = TxHash> {
        ReceiverStream::new(self.pool.pending_transactions_listener())
    }

    /// Returns a stream that yields all transactions emitted by the txpool.
    fn full_pending_transaction_stream(
        &self,
    ) -> impl Stream<Item = NewTransactionEvent<<Pool as TransactionPool>::Transaction>> {
        self.pool.new_pending_pool_transactions_listener()
    }
}

impl<Provider, Pool, Events, Network> EthPubSubInner<Provider, Pool, Events, Network>
where
    Provider: BlockReader + EvmEnvProvider + 'static,
    Events: CanonStateSubscriptions + 'static,
    Network: NetworkInfo + 'static,
    Pool: 'static,
{
    /// Returns a stream that yields all new RPC blocks.
    fn new_headers_stream(&self) -> impl Stream<Item = Header> {
        self.chain_events.canonical_state_stream().flat_map(|new_chain| {
            let headers = new_chain
                .committed()
                .map(|chain| chain.headers().collect::<Vec<_>>())
                .unwrap_or_default();
            futures::stream::iter(
                headers.into_iter().map(reth_rpc_types_compat::block::from_primitive_with_hash),
            )
        })
    }

    /// Returns a stream that yields all logs that match the given filter.
    fn log_stream(&self, filter: FilteredParams) -> impl Stream<Item = Log> {
        BroadcastStream::new(self.chain_events.subscribe_to_canonical_state())
            .map(move |canon_state| {
                canon_state.expect("new block subscription never ends; qed").block_receipts()
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
