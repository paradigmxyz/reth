//! `eth_` PubSub RPC handler implementation

use jsonrpsee::{types::SubscriptionResult, SubscriptionSink};
use reth_interfaces::{events::ChainEventSubscriptions, sync::SyncStateProvider};
use reth_primitives::{rpc::FilteredParams, TxHash};
use reth_provider::{BlockProvider, EvmEnvProvider};
use reth_rpc_api::EthPubSubApiServer;
use reth_rpc_types::{
    pubsub::{
        Params, PubSubSyncStatus, SubscriptionKind, SubscriptionResult as EthSubscriptionResult,
        SyncStatusMetadata,
    },
    Header,
};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::TransactionPool;
use tokio_stream::{
    wrappers::{ReceiverStream, UnboundedReceiverStream},
    Stream, StreamExt,
};

/// `Eth` pubsub RPC implementation.
///
/// This handles `eth_subscribe` RPC calls.
#[derive(Clone)]
pub struct EthPubSub<Client, Pool, Events, Network> {
    /// All nested fields bundled together.
    inner: EthPubSubInner<Client, Pool, Events, Network>,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Box<dyn TaskSpawner>,
}

// === impl EthPubSub ===

impl<Client, Pool, Events, Network> EthPubSub<Client, Pool, Events, Network> {
    /// Creates a new, shareable instance.
    ///
    /// Subscription tasks are spawned via [tokio::task::spawn]
    pub fn new(client: Client, pool: Pool, chain_events: Events, network: Network) -> Self {
        Self::with_spawner(client, pool, chain_events, network, Box::<TokioTaskExecutor>::default())
    }

    /// Creates a new, shareable instance.
    pub fn with_spawner(
        client: Client,
        pool: Pool,
        chain_events: Events,
        network: Network,
        subscription_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = EthPubSubInner { client, pool, chain_events, network };
        Self { inner, subscription_task_spawner }
    }
}

impl<Client, Pool, Events, Network> EthPubSubApiServer for EthPubSub<Client, Pool, Events, Network>
where
    Client: BlockProvider + EvmEnvProvider + SyncStateProvider + Clone + 'static,
    Pool: TransactionPool + 'static,
    Events: ChainEventSubscriptions + Clone + 'static,
    Network: SyncStateProvider + Clone + 'static,
{
    fn subscribe(
        &self,
        mut sink: SubscriptionSink,
        kind: SubscriptionKind,
        params: Option<Params>,
    ) -> SubscriptionResult {
        sink.accept()?;

        let pubsub = self.inner.clone();
        self.subscription_task_spawner.spawn(Box::pin(async move {
            handle_accepted(pubsub, sink, kind, params).await;
        }));

        Ok(())
    }
}

/// The actual handler for and accepted [`EthPubSub::subscribe`] call.
async fn handle_accepted<Client, Pool, Events, Network>(
    pubsub: EthPubSubInner<Client, Pool, Events, Network>,
    mut accepted_sink: SubscriptionSink,
    kind: SubscriptionKind,
    params: Option<Params>,
) where
    Client: BlockProvider + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
    Events: ChainEventSubscriptions + 'static,
    Network: SyncStateProvider + 'static,
{
    // if no params are provided, used default filter params
    let _params = match params {
        Some(Params::Logs(filter)) => FilteredParams::new(Some(*filter)),
        _ => FilteredParams::default(),
    };

    match kind {
        SubscriptionKind::NewHeads => {
            let stream = pubsub
                .into_new_headers_stream()
                .map(|block| EthSubscriptionResult::Header(Box::new(block.into())));
            accepted_sink.pipe_from_stream(stream).await;
        }
        SubscriptionKind::Logs => {
            // TODO subscribe new blocks -> fetch logs via bloom
        }
        SubscriptionKind::NewPendingTransactions => {
            let stream = pubsub
                .into_pending_transaction_stream()
                .map(EthSubscriptionResult::TransactionHash);
            accepted_sink.pipe_from_stream(stream).await;
        }
        SubscriptionKind::Syncing => {
            // TODO subscribe new blocks -> read is_syncing from network

            let mut block_subscription = pubsub.chain_events.subscribe_new_blocks();
            let initial_sync_status = pubsub.network.is_syncing();

            let initial_sub_res = if initial_sync_status {
                EthSubscriptionResult::SyncState(PubSubSyncStatus::Detailed(SyncStatusMetadata {
                    syncing: true,
                    starting_block: 0,
                    current_block: pubsub.client.chain_info().unwrap_or_default().best_number,
                    highest_block: None,
                }))
            } else {
                EthSubscriptionResult::SyncState(PubSubSyncStatus::Simple(false))
            };

            accepted_sink.send(&initial_sub_res).unwrap_or_default();

            while let Some(_) = block_subscription.recv().await {
                let sub_res = if pubsub.network.is_syncing() {
                    EthSubscriptionResult::SyncState(PubSubSyncStatus::Detailed(
                        SyncStatusMetadata {
                            syncing: true,
                            starting_block: 0,
                            current_block: pubsub.client.chain_info().unwrap().best_number,
                            highest_block: None,
                        },
                    ))
                } else {
                    EthSubscriptionResult::SyncState(PubSubSyncStatus::Simple(false))
                };

                if pubsub.network.is_syncing() != initial_sync_status {
                    accepted_sink.send(&initial_sub_res).unwrap_or_default();
                }
            }
        }
    }
}

impl<Client, Pool, Events, Network> std::fmt::Debug for EthPubSub<Client, Pool, Events, Network> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<Client, Pool, Events, Network> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Client,
    /// A type that allows to create new event subscriptions,
    chain_events: Events,
    /// The network.
    network: Network,
}

// == impl EthPubSubInner ===

impl<Client, Pool, Events, Network> EthPubSubInner<Client, Pool, Events, Network>
where
    Pool: TransactionPool + 'static,
{
    /// Returns a stream that yields all transactions emitted by the txpool.
    fn into_pending_transaction_stream(self) -> impl Stream<Item = TxHash> {
        ReceiverStream::new(self.pool.pending_transactions_listener())
    }
}

impl<Client, Pool, Events, Network> EthPubSubInner<Client, Pool, Events, Network>
where
    Client: BlockProvider + EvmEnvProvider + 'static,
    Events: ChainEventSubscriptions + 'static,
    Network: SyncStateProvider + 'static,
{
    /// Returns a stream that yields all new RPC blocks.
    fn into_new_headers_stream(self) -> impl Stream<Item = Header> {
        UnboundedReceiverStream::new(self.chain_events.subscribe_new_blocks()).map(|new_block| {
            Header::from_primitive_with_hash(new_block.header.as_ref().clone(), new_block.hash)
        })
    }
}
