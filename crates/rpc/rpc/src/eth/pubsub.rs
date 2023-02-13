//! `eth_` PubSub RPC handler implementation

use jsonrpsee::{types::SubscriptionResult, SubscriptionSink};
use reth_primitives::rpc::FilteredParams;
use reth_provider::BlockProvider;
use reth_rpc_api::EthPubSubApiServer;
use reth_rpc_types::pubsub::{Kind, Params};
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

/// `Eth` pubsub RPC implementation.
///
/// This handles
#[derive(Debug, Clone)]
pub struct EthPubSub<Client, Pool> {
    /// All nested fields bundled together.
    inner: Arc<EthPubSubInner<Client, Pool>>,
    /// The executor that's used to spawn subscription tasks.
    subscription_executor: TaskExecutor,
}

// === impl EthPubSub ===

impl<Client, Pool> EthPubSub<Client, Pool> {
    /// Creates a new, shareable instance.
    pub fn new(client: Arc<Client>, pool: Pool) -> Self {
        let inner = EthPubSubInner { client, pool };
        Self { inner: Arc::new(inner), subscription_executor: todo!() }
    }
}

impl<Client, Pool> EthPubSubApiServer for EthPubSub<Client, Pool>
where
    Client: BlockProvider + 'static,
    Pool: TransactionPool + 'static,
{
    fn subscribe(
        &self,
        mut sink: SubscriptionSink,
        kind: Kind,
        params: Option<Params>,
    ) -> SubscriptionResult {
        sink.accept()?;

        let pubsub = Arc::clone(&self.inner);
        self.subscription_executor
            .spawn(async move { handle_accepted(pubsub, sink, kind, params).await });

        todo!()
    }
}

/// The actual handler for and accepted [`EthPubSub::subscribe`] call.
async fn handle_accepted<Client, Pool>(
    pubsub: EthPubSubInner<Client, Pool>,
    _accepted_sink: SubscriptionSink,
    _kind: Kind,
    params: Option<Params>,
) {
    // if no params are provided, used default filter params
    let params = match params {
        Some(Params::Logs(filter)) => FilteredParams::new(Some(*filter)),
        _ => FilteredParams::default(),
    };
}

/// Container type `EthPubSub`
#[derive(Debug)]
struct EthPubSubInner<Client, Pool> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Arc<Client>,
    // TODO needs spawn access
}
