use std::{collections::HashMap, future::Future, sync::Arc};

use alloy_eips::BlockId;
use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use futures::StreamExt;
use jsonrpsee::{core::RpcResult, PendingSubscriptionSink, SubscriptionMessage, SubscriptionSink};
use jsonrpsee_types::ErrorObject;
use reth_chain_state::{CanonStateNotificationStream, CanonStateSubscriptions};
use reth_engine_primitives::ConsensusEngineEvent;
use reth_errors::RethResult;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_api::RethApiServer;
use reth_rpc_eth_types::{EthApiError, EthResult};
use reth_rpc_server_types::result::internal_rpc_err;
use reth_storage_api::{BlockReaderIdExt, ChangeSetReader, StateProviderFactory};
use reth_tasks::TaskSpawner;
use reth_tokio_util::{EventSender, EventStream};
use tokio::sync::oneshot;

/// `reth` API implementation.
///
/// This type provides the functionality for handling `reth` prototype RPC requests.
pub struct RethApi<Provider, N: NodePrimitives = reth_ethereum_primitives::EthPrimitives> {
    inner: Arc<RethApiInner<Provider, N>>,
}

// === impl RethApi ===

impl<Provider, N: NodePrimitives> RethApi<Provider, N> {
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Create a new instance of the [`RethApi`]
    pub fn new(
        provider: Provider,
        task_spawner: Box<dyn TaskSpawner>,
        engine_events: EventSender<ConsensusEngineEvent<N>>,
    ) -> Self {
        let inner = Arc::new(RethApiInner { provider, task_spawner, engine_events });
        Self { inner }
    }
}

impl<Provider, N> RethApi<Provider, N>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
    N: NodePrimitives,
{
    /// Executes the future on a new blocking task.
    async fn on_blocking_task<C, F, R>(&self, c: C) -> EthResult<R>
    where
        C: FnOnce(Self) -> F,
        F: Future<Output = EthResult<R>> + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let this = self.clone();
        let f = c(this);
        self.inner.task_spawner.spawn_blocking(Box::pin(async move {
            let res = f.await;
            let _ = tx.send(res);
        }));
        rx.await.map_err(|_| EthApiError::InternalEthError)?
    }

    /// Returns a map of addresses to changed account balanced for a particular block.
    pub async fn balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> EthResult<HashMap<Address, U256>> {
        self.on_blocking_task(|this| async move { this.try_balance_changes_in_block(block_id) })
            .await
    }

    fn try_balance_changes_in_block(&self, block_id: BlockId) -> EthResult<HashMap<Address, U256>> {
        let Some(block_number) = self.provider().block_number_for_id(block_id)? else {
            return Err(EthApiError::HeaderNotFound(block_id))
        };

        let state = self.provider().state_by_block_id(block_id)?;
        let accounts_before = self.provider().account_block_changeset(block_number)?;
        let hash_map = accounts_before.iter().try_fold(
            HashMap::default(),
            |mut hash_map, account_before| -> RethResult<_> {
                let current_balance = state.account_balance(&account_before.address)?;
                let prev_balance = account_before.info.map(|info| info.balance);
                if current_balance != prev_balance {
                    hash_map.insert(account_before.address, current_balance.unwrap_or_default());
                }
                Ok(hash_map)
            },
        )?;
        Ok(hash_map)
    }
}

#[async_trait]
impl<Provider, N> RethApiServer for RethApi<Provider, N>
where
    Provider: BlockReaderIdExt
        + ChangeSetReader
        + StateProviderFactory
        + CanonStateSubscriptions<Primitives = N>
        + 'static,
    N: NodePrimitives,
{
    /// Handler for `reth_getBalanceChangesInBlock`
    async fn reth_get_balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<HashMap<Address, U256>> {
        Ok(Self::balance_changes_in_block(self, block_id).await?)
    }

    /// Handler for `reth_subscribeChainNotifications`
    async fn reth_subscribe_chain_notifications(
        &self,
        pending: PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let stream = self.provider().canonical_state_stream();
        self.inner.task_spawner.spawn(Box::pin(async move {
            let _ = pipe_from_stream(sink, stream).await;
        }));

        Ok(())
    }

    /// Handler for `reth_subscribeLatestPersistedBlock`
    async fn reth_subscribe_latest_persisted_block(
        &self,
        pending: PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let stream = self.inner.engine_events.new_listener();
        self.inner.task_spawner.spawn(Box::pin(async move {
            let _ = pipe_persisted_blocks(sink, stream).await;
        }));

        Ok(())
    }
}

/// Pipes all stream items to the subscription sink.
async fn pipe_from_stream<N: NodePrimitives>(
    sink: SubscriptionSink,
    mut stream: CanonStateNotificationStream<N>,
) -> Result<(), ErrorObject<'static>> {
    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            }
            maybe_item = stream.next() => {
                let item = match maybe_item {
                    Some(item) => item,
                    None => {
                        // stream ended
                        break Ok(())
                    },
                };
                let msg = SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &item)
                    .map_err(|e| internal_rpc_err(e.to_string()))?;

                if sink.send(msg).await.is_err() {
                    break Ok(());
                }
            }
        }
    }
}

/// Pipes persisted block events to the subscription sink.
async fn pipe_persisted_blocks<N: NodePrimitives>(
    sink: SubscriptionSink,
    mut stream: EventStream<ConsensusEngineEvent<N>>,
) -> Result<(), ErrorObject<'static>> {
    loop {
        tokio::select! {
            _ = sink.closed() => {
                // connection dropped
                break Ok(())
            }
            maybe_event = stream.next() => {
                match maybe_event {
                    Some(ConsensusEngineEvent::LatestPersistedBlock(block)) => {
                        let msg = SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &block)
                            .map_err(|e| internal_rpc_err(e.to_string()))?;

                        if sink.send(msg).await.is_err() {
                            break Ok(());
                        }
                    }
                    Some(_) => {
                        // Ignore other events
                    }
                    None => {
                        // stream ended
                        break Ok(())
                    }
                }
            }
        }
    }
}

impl<Provider, N: NodePrimitives> std::fmt::Debug for RethApi<Provider, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RethApi").finish_non_exhaustive()
    }
}

impl<Provider, N: NodePrimitives> Clone for RethApi<Provider, N> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct RethApiInner<Provider, N: NodePrimitives> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
    /// Engine events sender for subscribing to consensus engine events.
    engine_events: EventSender<ConsensusEngineEvent<N>>,
}
