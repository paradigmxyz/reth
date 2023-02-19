//! `eth_` PubSub RPC handler implementation

use jsonrpsee::{types::SubscriptionResult, SubscriptionSink};
use reth_interfaces::events::ChainEventSubscriptions;
use reth_primitives::{rpc::FilteredParams, TxHash};
use reth_provider::BlockProvider;
use reth_rpc_api::EthPubSubApiServer;
use reth_rpc_types::{
    pubsub::{Params, SubscriptionKind, SubscriptionResult as EthSubscriptionResult},
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
/// This handles
pub struct EthPubSub<Client, Pool, Events> {
    /// All nested fields bundled together.
    inner: EthPubSubInner<Client, Pool, Events>,
    /// The type that's used to spawn subscription tasks.
    subscription_task_spawner: Box<dyn TaskSpawner>,
}

// === impl EthPubSub ===

impl<Client, Pool, Events> EthPubSub<Client, Pool, Events> {
    /// Creates a new, shareable instance.
    ///
    /// Subscription tasks are spawned via [tokio::task::spawn]
    pub fn new(client: Client, pool: Pool, chain_events: Events) -> Self {
        Self::with_spawner(client, pool, chain_events, Box::<TokioTaskExecutor>::default())
    }

    /// Creates a new, shareable instance.
    pub fn with_spawner(
        client: Client,
        pool: Pool,
        chain_events: Events,
        subscription_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = EthPubSubInner { client, pool, chain_events };
        Self { inner, subscription_task_spawner }
    }
}

impl<Client, Pool, Events> EthPubSubApiServer for EthPubSub<Client, Pool, Events>
where
    Client: BlockProvider + Clone + 'static,
    Pool: TransactionPool + 'static,
    Events: ChainEventSubscriptions + Clone + 'static,
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
async fn handle_accepted<Client, Pool, Events>(
    pubsub: EthPubSubInner<Client, Pool, Events>,
    mut accepted_sink: SubscriptionSink,
    kind: SubscriptionKind,
    params: Option<Params>,
) where
    Client: BlockProvider + 'static,
    Pool: TransactionPool + 'static,
    Events: ChainEventSubscriptions + 'static,
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
        }
    }
}

impl<Client, Pool, Events> std::fmt::Debug for EthPubSub<Client, Pool, Events> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthPubSub").finish_non_exhaustive()
    }
}

/// Container type `EthPubSub`
#[derive(Clone)]
struct EthPubSubInner<Client, Pool, Events> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Client,
    /// A type that allows to create new event subscriptions,
    chain_events: Events,
}

// == impl EthPubSubInner ===

impl<Client, Pool, Events> EthPubSubInner<Client, Pool, Events>
where
    Pool: TransactionPool + 'static,
{
    /// Returns a stream that yields all transactions emitted by the txpool.
    fn into_pending_transaction_stream(self) -> impl Stream<Item = TxHash> {
        ReceiverStream::new(self.pool.pending_transactions_listener())
    }
}

impl<Client, Pool, Events> EthPubSubInner<Client, Pool, Events>
where
    Client: BlockProvider + 'static,
    Events: ChainEventSubscriptions + 'static,
{
    /// Returns a stream that yields all new RPC blocks.
    fn into_new_headers_stream(self) -> impl Stream<Item = Header> {
        UnboundedReceiverStream::new(self.chain_events.subscribe_new_blocks()).map(|new_block| {
            Header::from_primitive_with_hash(new_block.header.as_ref().clone(), new_block.hash)
        })
    }
}
