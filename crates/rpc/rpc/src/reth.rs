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
use reth_errors::RethResult;
use reth_evm::{execute::Executor, ConfigureEvm};
use reth_execution_types::ExecutionOutcome;
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_rpc_api::RethApiServer;
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
    N::Receipt: Serialize,
{
    /// Re-executes one or more consecutive blocks and returns the execution outcome.
    pub async fn block_execution_outcome(
        &self,
        block_id: BlockId,
        count: Option<U64>,
    ) -> EthResult<Option<ExecutionOutcome<N::Receipt>>> {
        let permit = self
            .inner
            .blocking_task_guard
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| EthApiError::InternalEthError)?;
        self.on_blocking_task(move |this| async move {
            let _permit = permit;
            this.try_block_execution_outcome(block_id, count)
        })
        .await
    }

    fn try_block_execution_outcome(
        &self,
        block_id: BlockId,
        count: Option<U64>,
    ) -> EthResult<Option<ExecutionOutcome<N::Receipt>>> {
        let Some(start_block) = self.provider().block_number_for_id(block_id)? else {
            return Ok(None)
        };

        if start_block == 0 {
            return Err(EthApiError::InvalidParams(
                "cannot re-execute genesis block (no parent state)".to_string(),
            ));
        }

        const MAX_BLOCK_COUNT: u64 = 256;

        let block_count = count.map(|c| c.to::<u64>()).unwrap_or(1).clamp(1, MAX_BLOCK_COUNT);

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
    Provider::Primitives: NodePrimitives<Receipt = reth_ethereum_primitives::Receipt>,
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
    ) -> RpcResult<Option<ExecutionOutcome>> {
        Ok(Self::block_execution_outcome(self, block_id, count).await?)
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
    use alloy_evm::block::BlockExecutionResult;
    use alloy_primitives::{address, B256, U256};
    use reth_ethereum_primitives::Receipt;

    use revm::{
        database::{
            states::{BundleState, StorageSlot},
            AccountStatus, BundleAccount,
        },
        state::AccountInfo,
    };
    use std::collections::HashMap;

    fn test_receipt() -> Receipt {
        Receipt {
            tx_type: alloy_consensus::TxType::Eip1559,
            success: true,
            cumulative_gas_used: 21000,
            logs: vec![],
        }
    }

    #[test]
    fn execution_outcome_multi_block() {
        let addr_a = address!("0x0000000000000000000000000000000000000001");
        let addr_b = address!("0x0000000000000000000000000000000000000002");

        let mut state_a = HashMap::default();
        state_a.insert(
            addr_a,
            BundleAccount {
                info: Some(AccountInfo {
                    nonce: 1,
                    balance: U256::from(500),
                    code_hash: B256::ZERO,
                    ..Default::default()
                }),
                original_info: None,
                storage: Default::default(),
                status: AccountStatus::Changed,
            },
        );

        let mut state_b = HashMap::default();
        state_b.insert(
            addr_b,
            BundleAccount {
                info: Some(AccountInfo {
                    nonce: 3,
                    balance: U256::from(900),
                    code_hash: B256::ZERO,
                    ..Default::default()
                }),
                original_info: None,
                storage: Default::default(),
                status: AccountStatus::Changed,
            },
        );

        let mut bundle = BundleState {
            state: state_a,
            contracts: Default::default(),
            reverts: Default::default(),
            state_size: 0,
            reverts_size: 0,
        };
        bundle.extend(BundleState {
            state: state_b,
            contracts: Default::default(),
            reverts: Default::default(),
            state_size: 0,
            reverts_size: 0,
        });

        let results = vec![
            BlockExecutionResult {
                receipts: vec![test_receipt()],
                requests: Default::default(),
                gas_used: 21000,
                blob_gas_used: 0,
            },
            BlockExecutionResult {
                receipts: vec![test_receipt(), test_receipt()],
                requests: Default::default(),
                gas_used: 42000,
                blob_gas_used: 131072,
            },
        ];

        let outcome = ExecutionOutcome::from_blocks(5, bundle, results);

        assert_eq!(outcome.first_block, 5);
        assert_eq!(outcome.receipts.len(), 2);
        assert_eq!(outcome.receipts[0].len(), 1);
        assert_eq!(outcome.receipts[1].len(), 2);
        assert_eq!(outcome.requests.len(), 2);
        assert!(outcome.bundle.account(&addr_a).is_some());
        assert!(outcome.bundle.account(&addr_b).is_some());
        assert_eq!(outcome.bundle.account(&addr_a).unwrap().info.as_ref().unwrap().nonce, 1);
        assert_eq!(
            outcome.bundle.account(&addr_b).unwrap().info.as_ref().unwrap().balance,
            U256::from(900)
        );
    }

    #[test]
    fn execution_outcome_serde_roundtrip_with_state() {
        let addr = address!("0x0000000000000000000000000000000000000042");
        let slot = U256::from(7);
        let value = U256::from(999);

        let mut storage = HashMap::default();
        storage.insert(slot, StorageSlot::new(value));

        let mut state = HashMap::default();
        state.insert(
            addr,
            BundleAccount {
                info: Some(AccountInfo {
                    nonce: 10,
                    balance: U256::from(5_000),
                    code_hash: B256::ZERO,
                    ..Default::default()
                }),
                original_info: None,
                storage,
                status: AccountStatus::Changed,
            },
        );

        let raw_bytes: &[u8] = &[0x60, 0x00, 0x60, 0x00, 0xfd];
        let code =
            revm::bytecode::Bytecode::new_raw(alloy_primitives::Bytes::from_static(raw_bytes));
        let code_hash = B256::from(alloy_primitives::keccak256(code.original_byte_slice()));
        let mut contracts = HashMap::default();
        contracts.insert(code_hash, code);

        let outcome = ExecutionOutcome::new(
            BundleState {
                state,
                contracts,
                reverts: Default::default(),
                state_size: 0,
                reverts_size: 0,
            },
            vec![vec![test_receipt()]],
            1,
            vec![Default::default()],
        );

        let json = serde_json::to_value(&outcome).unwrap();
        let deserialized: ExecutionOutcome = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.first_block, 1);
        assert_eq!(deserialized.receipts.len(), 1);
        assert_eq!(deserialized.receipts[0][0], test_receipt());

        let account = deserialized.bundle.account(&addr).unwrap();
        assert_eq!(account.info.as_ref().unwrap().nonce, 10);
        assert_eq!(account.info.as_ref().unwrap().balance, U256::from(5_000));
        assert_eq!(account.storage.get(&slot).unwrap().present_value, value,);

        assert!(deserialized.bundle.contracts.contains_key(&code_hash));
    }
}
