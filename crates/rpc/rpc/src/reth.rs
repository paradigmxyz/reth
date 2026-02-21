use std::{future::Future, pin::Pin, sync::Arc, time::Instant};

use alloy_consensus::BlockHeader;
use alloy_eips::BlockId;
use alloy_primitives::{map::AddressMap, U256, U64};
use alloy_rpc_types_engine::PayloadStatus;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use jsonrpsee::{core::RpcResult, PendingSubscriptionSink, SubscriptionMessage, SubscriptionSink};
use reth_chain_state::{
    CanonStateNotification, CanonStateSubscriptions, ForkChoiceSubscriptions,
    PersistedBlockSubscriptions,
};
use reth_engine_primitives::{ConsensusEngineHandle, NewPayloadTimings};
use reth_errors::RethResult;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_execution_types::ExecutionOutcome;
use reth_payload_primitives::PayloadTypes;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_rpc_api::{RethApiServer, RethPayloadStatus};
use reth_rpc_eth_types::{EthApiError, EthResult};
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, ChangeSetReader, StateProviderFactory, TransactionVariant,
};
use reth_tasks::{pool::BlockingTaskGuard, Runtime};
use serde::Serialize;
use tokio::sync::oneshot;

/// `reth` API implementation.
///
/// This type provides the functionality for handling `reth` prototype RPC requests.
pub struct RethApi<Provider, EvmConfig, ExecutionData> {
    inner: Arc<RethApiInner<Provider, EvmConfig, ExecutionData>>,
}

// === impl RethApi ===

impl<Provider, EvmConfig, ExecutionData> RethApi<Provider, EvmConfig, ExecutionData> {
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// The evm config.
    pub fn evm_config(&self) -> &EvmConfig {
        &self.inner.evm_config
    }

    /// Create a new instance of the [`RethApi`]
    pub fn new(
        provider: Provider,
        evm_config: EvmConfig,
        blocking_task_guard: BlockingTaskGuard,
        task_spawner: Runtime,
        beacon_engine_handle: Option<Arc<dyn RethNewPayloadHandle<ExecutionData>>>,
    ) -> Self {
        let inner = Arc::new(RethApiInner {
            provider,
            evm_config,
            blocking_task_guard,
            task_spawner,
            beacon_engine_handle,
        });
        Self { inner }
    }
}

impl<Provider, EvmConfig, ExecutionData> RethApi<Provider, EvmConfig, ExecutionData>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
    EvmConfig: Send + Sync + 'static,
    ExecutionData: Send + Sync + 'static,
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
        self.inner.task_spawner.spawn_blocking_task(async move {
            let res = f.await;
            let _ = tx.send(res);
        });
        rx.await.map_err(|_| EthApiError::InternalEthError)?
    }

    /// Returns a map of addresses to changed account balanced for a particular block.
    pub async fn balance_changes_in_block(&self, block_id: BlockId) -> EthResult<AddressMap<U256>> {
        self.on_blocking_task(|this| async move { this.try_balance_changes_in_block(block_id) })
            .await
    }

    fn try_balance_changes_in_block(&self, block_id: BlockId) -> EthResult<AddressMap<U256>> {
        let Some(block_number) = self.provider().block_number_for_id(block_id)? else {
            return Err(EthApiError::HeaderNotFound(block_id))
        };

        let state = self.provider().state_by_block_id(block_id)?;
        let accounts_before = self.provider().account_block_changeset(block_number)?;
        let hash_map = accounts_before.iter().try_fold(
            AddressMap::default(),
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

impl<N, Provider, EvmConfig, ExecutionData> RethApi<Provider, EvmConfig, ExecutionData>
where
    N: NodePrimitives,
    Provider: BlockReaderIdExt
        + ChangeSetReader
        + StateProviderFactory
        + BlockReader<Block = N::Block>
        + CanonStateSubscriptions<Primitives = N>
        + 'static,
    EvmConfig: ConfigureEvm<Primitives = N> + 'static,
    ExecutionData: Send + Sync + 'static,
{
    /// Re-executes one or more consecutive blocks and returns the execution outcome.
    pub async fn block_execution_outcome(
        &self,
        block_id: BlockId,
        count: Option<U64>,
    ) -> EthResult<Option<ExecutionOutcome<N::Receipt>>> {
        const MAX_BLOCK_COUNT: u64 = 128;

        let block_count = count.map(|c| c.to::<u64>()).unwrap_or(1);
        if block_count == 0 || block_count > MAX_BLOCK_COUNT {
            return Err(EthApiError::InvalidParams(format!(
                "block count must be between 1 and {MAX_BLOCK_COUNT}, got {block_count}"
            )))
        }

        let permit = self
            .inner
            .blocking_task_guard
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| EthApiError::InternalEthError)?;
        self.on_blocking_task(move |this| async move {
            let _permit = permit;
            this.try_block_execution_outcome(block_id, block_count)
        })
        .await
    }

    fn try_block_execution_outcome(
        &self,
        block_id: BlockId,
        block_count: u64,
    ) -> EthResult<Option<ExecutionOutcome<N::Receipt>>> {
        let Some(start_block) = self.provider().block_number_for_id(block_id)? else {
            return Ok(None)
        };

        let state_provider = self.provider().history_by_block_number(start_block - 1)?;
        let db = reth_revm::database::StateProviderDatabase::new(&state_provider);

        let mut blocks = Vec::with_capacity(block_count as usize);
        for block_number in start_block..start_block + block_count {
            let Some(block) = self
                .provider()
                .recovered_block(block_number.into(), TransactionVariant::WithHash)?
            else {
                if block_number == start_block {
                    return Ok(None)
                }
                break;
            };
            blocks.push(block);
        }

        let outcome = self.evm_config().executor(db).execute_batch(&blocks).map_err(
            |e: reth_evm::execute::BlockExecutionError| {
                EthApiError::Internal(reth_errors::RethError::Other(e.into()))
            },
        )?;

        Ok(Some(outcome))
    }
}

impl<Provider, EvmConfig, ExecutionData> RethApi<Provider, EvmConfig, ExecutionData>
where
    Provider: 'static,
    ExecutionData: Send + Sync + 'static,
{
    /// Waits for persistence, execution cache, and sparse trie locks before processing.
    ///
    /// Used by `reth_newPayload` endpoint.
    pub async fn reth_new_payload(
        &self,
        payload: ExecutionData,
    ) -> Result<RethPayloadStatus, jsonrpsee::types::ErrorObject<'static>> {
        let Some(beacon_engine_handle) = &self.inner.beacon_engine_handle else {
            return Err(jsonrpsee::types::error::ErrorObject::owned(
                jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                "beacon engine handle not available",
                None::<()>,
            ))
        };
        let start = Instant::now();
        let (status, timings) =
            beacon_engine_handle.reth_new_payload(payload).await.map_err(|e| {
                jsonrpsee::types::error::ErrorObject::owned(
                    jsonrpsee::types::error::INTERNAL_ERROR_CODE,
                    e.to_string(),
                    None::<()>,
                )
            })?;
        tracing::debug!(target: "rpc::reth", latency=?start.elapsed(), "Served reth_newPayload");
        Ok(RethPayloadStatus {
            status,
            latency_us: timings.latency.as_micros() as u64,
            persistence_wait_us: timings.persistence_wait.map(|d| d.as_micros() as u64),
            execution_cache_wait_us: timings.execution_cache_wait.as_micros() as u64,
            sparse_trie_wait_us: timings.sparse_trie_wait.as_micros() as u64,
        })
    }
}

#[async_trait]
impl<Provider, EvmConfig, ExecutionData> RethApiServer<ExecutionData>
    for RethApi<Provider, EvmConfig, ExecutionData>
where
    Provider: BlockReaderIdExt
        + ChangeSetReader
        + StateProviderFactory
        + BlockReader<Block = <Provider::Primitives as NodePrimitives>::Block>
        + CanonStateSubscriptions
        + ForkChoiceSubscriptions<Header = <Provider::Primitives as NodePrimitives>::BlockHeader>
        + PersistedBlockSubscriptions
        + 'static,
    EvmConfig: ConfigureEvm<Primitives = Provider::Primitives> + 'static,
    ExecutionData: Send + Sync + 'static,
{
    /// Handler for `reth_getBalanceChangesInBlock`
    async fn reth_get_balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<AddressMap<U256>> {
        Ok(Self::balance_changes_in_block(self, block_id).await?)
    }

    /// Handler for `reth_getBlockExecutionOutcome`
    async fn reth_get_block_execution_outcome(
        &self,
        block_id: BlockId,
        count: Option<U64>,
    ) -> RpcResult<Option<serde_json::Value>> {
        let outcome = Self::block_execution_outcome(self, block_id, count).await?;
        match outcome {
            Some(outcome) => {
                let value = serde_json::to_value(&outcome).map_err(|e| {
                    EthApiError::Internal(reth_errors::RethError::msg(e.to_string()))
                })?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    /// Handler for `reth_subscribeChainNotifications`
    async fn reth_subscribe_chain_notifications(
        &self,
        pending: PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let stream = self.provider().canonical_state_stream();
        self.inner.task_spawner.spawn_task(pipe_from_stream(sink, stream));

        Ok(())
    }

    /// Handler for `reth_subscribePersistedBlock`
    async fn reth_subscribe_persisted_block(
        &self,
        pending: PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let stream = self.provider().persisted_block_stream();
        self.inner.task_spawner.spawn_task(pipe_from_stream(sink, stream));

        Ok(())
    }

    /// Handler for `reth_subscribeFinalizedChainNotifications`
    async fn reth_subscribe_finalized_chain_notifications(
        &self,
        pending: PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        let sink = pending.accept().await?;
        let canon_stream = self.provider().canonical_state_stream();
        let finalized_stream = self.provider().finalized_block_stream();
        self.inner.task_spawner.spawn_task(finalized_chain_notifications(
            sink,
            canon_stream,
            finalized_stream,
        ));

        Ok(())
    }

    /// Handler for `reth_newPayload`
    async fn reth_new_payload(&self, payload: ExecutionData) -> RpcResult<RethPayloadStatus> {
        Ok(Self::reth_new_payload(self, payload).await?)
    }
}

/// A type-erased handle for sending `reth_newPayload` requests.
///
/// This abstracts away the full `ConsensusEngineHandle<PayloadT>` so that `RethApi`
/// only needs to be generic over `ExecutionData`.
pub trait RethNewPayloadHandle<ExecutionData>: Send + Sync + 'static {
    /// Sends a `reth_newPayload` request and returns the payload status with timing info.
    fn reth_new_payload(
        &self,
        payload: ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (PayloadStatus, NewPayloadTimings),
                        reth_engine_primitives::BeaconOnNewPayloadError,
                    >,
                > + Send,
        >,
    >;
}

impl<T> RethNewPayloadHandle<T::ExecutionData> for ConsensusEngineHandle<T>
where
    T: PayloadTypes,
{
    fn reth_new_payload(
        &self,
        payload: T::ExecutionData,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        (PayloadStatus, NewPayloadTimings),
                        reth_engine_primitives::BeaconOnNewPayloadError,
                    >,
                > + Send,
        >,
    > {
        let this = self.clone();
        Box::pin(async move { this.reth_new_payload(payload).await })
    }
}

/// Pipes all stream items to the subscription sink.
async fn pipe_from_stream<S, T>(sink: SubscriptionSink, mut stream: S)
where
    S: Stream<Item = T> + Unpin,
    T: Serialize,
{
    loop {
        tokio::select! {
            _ = sink.closed() => {
                break
            }
            maybe_item = stream.next() => {
                let Some(item) = maybe_item else {
                    break
                };
                let msg = match SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &item) {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::error!(target: "rpc::reth", %err, "Failed to serialize subscription message");
                        break
                    }
                };
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Buffers committed chain notifications and emits them when a new finalized block is received.
async fn finalized_chain_notifications<N>(
    sink: SubscriptionSink,
    mut canon_stream: reth_chain_state::CanonStateNotificationStream<N>,
    mut finalized_stream: reth_chain_state::ForkChoiceStream<SealedHeader<N::BlockHeader>>,
) where
    N: NodePrimitives,
{
    let mut buffered: Vec<CanonStateNotification<N>> = Vec::new();

    loop {
        tokio::select! {
            _ = sink.closed() => {
                break
            }
            maybe_canon = canon_stream.next() => {
                let Some(notification) = maybe_canon else { break };
                match &notification {
                    CanonStateNotification::Commit { .. } => {
                        buffered.push(notification);
                    }
                    CanonStateNotification::Reorg { .. } => {
                        buffered.clear();
                    }
                }
            }
            maybe_finalized = finalized_stream.next() => {
                let Some(finalized_header) = maybe_finalized else { break };
                let finalized_num = finalized_header.number();

                let mut committed = Vec::new();
                buffered.retain(|n| {
                    if *n.committed().range().end() <= finalized_num {
                        committed.push(n.clone());
                        false
                    } else {
                        true
                    }
                });

                if committed.is_empty() {
                    continue;
                }

                committed.sort_by_key(|n| *n.committed().range().start());

                let msg = match SubscriptionMessage::new(
                    sink.method_name(),
                    sink.subscription_id(),
                    &committed,
                ) {
                    Ok(msg) => msg,
                    Err(err) => {
                        tracing::error!(target: "rpc::reth", %err, "Failed to serialize finalized chain notification");
                        break
                    }
                };
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        }
    }
}

impl<Provider, EvmConfig, ExecutionData: 'static> std::fmt::Debug
    for RethApi<Provider, EvmConfig, ExecutionData>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RethApi").finish_non_exhaustive()
    }
}

impl<Provider, EvmConfig, ExecutionData: 'static> Clone
    for RethApi<Provider, EvmConfig, ExecutionData>
{
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct RethApiInner<Provider, EvmConfig, ExecutionData> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The EVM configuration used to create block executors.
    evm_config: EvmConfig,
    /// Guard to restrict the number of concurrent block re-execution requests.
    blocking_task_guard: BlockingTaskGuard,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Runtime,
    /// Optional beacon engine handle for `reth_newPayload`.
    beacon_engine_handle: Option<Arc<dyn RethNewPayloadHandle<ExecutionData>>>,
}
