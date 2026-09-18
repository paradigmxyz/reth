use std::{future::Future, sync::Arc};

use alloy_consensus::BlockHeader;
use alloy_eips::BlockId;
use alloy_primitives::{map::AddressMap, U256, U64};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use jsonrpsee::{core::RpcResult, PendingSubscriptionSink, SubscriptionMessage, SubscriptionSink};
use reth_chain_state::{
    CanonStateNotification, CanonStateSubscriptions, ForkChoiceSubscriptions,
    PersistedBlockSubscriptions,
};
use reth_errors::{RethError, RethResult};
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_rpc_api::{RethApiServer, RethJitAction};
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
pub struct RethApi<Provider, EvmConfig> {
    inner: Arc<RethApiInner<Provider, EvmConfig>>,
}

// === impl RethApi ===

impl<Provider, EvmConfig> RethApi<Provider, EvmConfig> {
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
    ) -> Self {
        let inner =
            Arc::new(RethApiInner { provider, evm_config, blocking_task_guard, task_spawner });
        Self { inner }
    }
}

impl<Provider, EvmConfig> RethApi<Provider, EvmConfig>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
    EvmConfig: Send + Sync + 'static,
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
        self.on_blocking_task(async move |this| this.try_balance_changes_in_block(block_id)).await
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

impl<N, Provider, EvmConfig> RethApi<Provider, EvmConfig>
where
    N: NodePrimitives,
    Provider: BlockReaderIdExt
        + ChangeSetReader
        + StateProviderFactory
        + BlockReader<Block = N::Block>
        + CanonStateSubscriptions<Primitives = N>
        + 'static,
    EvmConfig: ConfigureEvm<Primitives = N> + 'static,
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
        self.on_blocking_task(async move |this| {
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

        if start_block == 0 {
            return Ok(Some(ExecutionOutcome::default()))
        }

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

#[async_trait]
impl<Provider, EvmConfig> RethApiServer for RethApi<Provider, EvmConfig>
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

    /// Handler for `reth_jit`
    async fn reth_jit(&self, action: RethJitAction) -> RpcResult<()> {
        let Some(jit_backend) = self.evm_config().jit_backend() else {
            return Ok(());
        };

        match action {
            RethJitAction::Enable => jit_backend
                .set_enabled(true)
                .map_err(|err| EthApiError::Internal(RethError::msg(err)))?,
            RethJitAction::Disable => jit_backend
                .set_enabled(false)
                .map_err(|err| EthApiError::Internal(RethError::msg(err)))?,
            RethJitAction::Pause => jit_backend.pause(),
            RethJitAction::Unpause => jit_backend.resume(),
            RethJitAction::Clear => jit_backend.clear(),
        }

        Ok(())
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
    let mut buffer = FinalizedNotificationBuffer::default();

    loop {
        tokio::select! {
            _ = sink.closed() => {
                break
            }
            maybe_canon = canon_stream.next() => {
                let Some(notification) = maybe_canon else { break };
                buffer.on_canonical_state(notification);
            }
            maybe_finalized = finalized_stream.next() => {
                let Some(finalized_header) = maybe_finalized else { break };
                let committed = buffer.on_finalized(finalized_header.number());

                if committed.is_empty() {
                    continue;
                }

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

/// Holds canonical chain segments that have been committed but not finalized yet.
#[derive(Debug)]
struct FinalizedNotificationBuffer<N: NodePrimitives> {
    buffered: Vec<CanonStateNotification<N>>,
}

impl<N: NodePrimitives> Default for FinalizedNotificationBuffer<N> {
    fn default() -> Self {
        Self { buffered: Vec::new() }
    }
}

impl<N: NodePrimitives> FinalizedNotificationBuffer<N> {
    /// Records a canonical state change.
    ///
    /// A reorg only invalidates the buffered blocks it reverted; everything below the fork point
    /// stays canonical and is still awaiting finalization. The blocks committed by the reorg are
    /// buffered like any other commit.
    fn on_canonical_state(&mut self, notification: CanonStateNotification<N>) {
        match notification {
            CanonStateNotification::Commit { .. } => self.buffered.push(notification),
            CanonStateNotification::Reorg { old, new } => {
                let fork = *old.range().start();
                self.buffered.retain_mut(|buffered| {
                    let range = buffered.committed().range();
                    if *range.end() < fork {
                        return true
                    }
                    if *range.start() >= fork {
                        return false
                    }
                    *buffered = CanonStateNotification::Commit {
                        new: Arc::new(truncate_chain(&buffered.committed(), fork)),
                    };
                    true
                });
                self.buffered.push(CanonStateNotification::Commit { new });
            }
        }
    }

    /// Drains all buffered segments that are fully covered by the given finalized block number,
    /// ordered by block number.
    fn on_finalized(&mut self, finalized: u64) -> Vec<CanonStateNotification<N>> {
        let mut committed = Vec::new();
        self.buffered.retain(|n| {
            if *n.committed().range().end() <= finalized {
                committed.push(n.clone());
                false
            } else {
                true
            }
        });
        committed.sort_by_key(|n| *n.committed().range().start());
        committed
    }
}

/// Returns the part of the chain below the given block number.
fn truncate_chain<N: NodePrimitives>(chain: &Chain<N>, below: u64) -> Chain<N> {
    let (blocks, mut execution_outcome, mut trie_data) = chain.clone().into_inner();
    execution_outcome.revert_to(below - 1);
    trie_data.split_off(&below);
    Chain::new(
        blocks.into_blocks().filter(|b| b.header().number() < below),
        execution_outcome,
        trie_data,
    )
}

impl<Provider, EvmConfig> std::fmt::Debug for RethApi<Provider, EvmConfig> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RethApi").finish_non_exhaustive()
    }
}

impl<Provider, EvmConfig> Clone for RethApi<Provider, EvmConfig> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct RethApiInner<Provider, EvmConfig> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The EVM configuration used to create block executors.
    evm_config: EvmConfig,
    /// Guard to restrict the number of concurrent block re-execution requests.
    blocking_task_guard: BlockingTaskGuard,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Runtime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_testing_utils::generators::{self, BlockParams};
    use std::{collections::BTreeMap, ops::RangeInclusive};

    fn chain(range: RangeInclusive<u64>, parent: B256) -> Arc<Chain<EthPrimitives>> {
        let mut rng = generators::rng();
        let first_block = *range.start();
        let mut parent = parent;
        let blocks = range.map(|number| {
            let block = generators::random_block(
                &mut rng,
                number,
                BlockParams { parent: Some(parent), tx_count: Some(0), ..Default::default() },
            );
            parent = block.hash();
            block.try_recover().unwrap()
        });
        Arc::new(Chain::new(
            blocks,
            ExecutionOutcome { first_block, ..Default::default() },
            BTreeMap::new(),
        ))
    }

    fn hashes(notifications: &[CanonStateNotification<EthPrimitives>]) -> Vec<(u64, B256)> {
        notifications
            .iter()
            .flat_map(|n| {
                n.committed().blocks_iter().map(|b| (b.number(), b.hash())).collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn finalized_buffer_keeps_unreverted_blocks_on_reorg() {
        let mut buffer = FinalizedNotificationBuffer::<EthPrimitives>::default();

        let first = chain(1..=3, B256::ZERO);
        let second = chain(4..=6, first.tip().hash());
        buffer.on_canonical_state(CanonStateNotification::Commit { new: first.clone() });
        buffer.on_canonical_state(CanonStateNotification::Commit { new: second.clone() });

        // blocks 5 and 6 are replaced, 1..=4 stay canonical
        let old = Arc::new(truncate_chain(&second, 5));
        let reverted = chain(5..=6, old.tip().hash());
        assert_eq!(reverted.range(), 5..=6);
        let new = chain(5..=7, old.tip().hash());
        buffer
            .on_canonical_state(CanonStateNotification::Reorg { old: reverted, new: new.clone() });

        let finalized = buffer.on_finalized(4);
        let mut expected = hashes(&[CanonStateNotification::Commit { new: first }]);
        expected.extend(hashes(&[CanonStateNotification::Commit { new: old }]));
        assert_eq!(hashes(&finalized), expected);
        assert_eq!(finalized[1].committed().execution_outcome().first_block, 4);

        let finalized = buffer.on_finalized(7);
        assert_eq!(hashes(&finalized), hashes(&[CanonStateNotification::Commit { new }]));
        assert!(buffer.on_finalized(u64::MAX).is_empty());
    }

    #[test]
    fn finalized_buffer_drops_fully_reverted_segments() {
        let mut buffer = FinalizedNotificationBuffer::<EthPrimitives>::default();

        let first = chain(1..=2, B256::ZERO);
        let reverted = chain(3..=4, first.tip().hash());
        buffer.on_canonical_state(CanonStateNotification::Commit { new: first.clone() });
        buffer.on_canonical_state(CanonStateNotification::Commit { new: reverted.clone() });

        let new = chain(3..=5, first.tip().hash());
        buffer
            .on_canonical_state(CanonStateNotification::Reorg { old: reverted, new: new.clone() });

        let finalized = buffer.on_finalized(5);
        assert_eq!(
            hashes(&finalized),
            hashes(&[
                CanonStateNotification::Commit { new: first },
                CanonStateNotification::Commit { new }
            ])
        );
    }
}
