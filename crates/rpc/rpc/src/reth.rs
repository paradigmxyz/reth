use crate::eth::error::{EthApiError, EthResult};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_interfaces::Result;
use reth_primitives::{Address, BlockId, U256};
use reth_provider::{BlockReaderIdExt, ChangeSetReader, StateProviderFactory};
use reth_rpc_api::RethApiServer;
use reth_tasks::TaskSpawner;
use std::{collections::HashMap, future::Future, sync::Arc};
use tokio::sync::oneshot;

/// `reth` API implementation.
///
/// This type provides the functionality for handling `reth` prototype RPC requests.
pub struct RethApi<Provider> {
    inner: Arc<RethApiInner<Provider>>,
}

// === impl RethApi ===

impl<Provider> RethApi<Provider> {
    /// The provider that can interact with the chain.
    pub fn provider(&self) -> &Provider {
        &self.inner.provider
    }

    /// Create a new instance of the [RethApi]
    pub fn new(provider: Provider, task_spawner: Box<dyn TaskSpawner>) -> Self {
        let inner = Arc::new(RethApiInner { provider, task_spawner });
        Self { inner }
    }
}

impl<Provider> RethApi<Provider>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
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
        let block_id = block_id;
        let Some(block_number) = self.provider().block_number_for_id(block_id)? else {
            return Err(EthApiError::UnknownBlockNumber)
        };

        let state = self.provider().state_by_block_id(block_id)?;
        let accounts_before = self.provider().account_block_changeset(block_number)?;
        let hash_map = accounts_before.iter().try_fold(
            HashMap::new(),
            |mut hash_map, account_before| -> Result<_> {
                let current_balance = state.account_balance(account_before.address)?;
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
impl<Provider> RethApiServer for RethApi<Provider>
where
    Provider: BlockReaderIdExt + ChangeSetReader + StateProviderFactory + 'static,
{
    /// Handler for `reth_getBalanceChangesInBlock`
    async fn reth_get_balance_changes_in_block(
        &self,
        block_id: BlockId,
    ) -> RpcResult<HashMap<Address, U256>> {
        Ok(RethApi::balance_changes_in_block(self, block_id).await?)
    }
}

impl<Provider> std::fmt::Debug for RethApi<Provider> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RethApi").finish_non_exhaustive()
    }
}

impl<Provider> Clone for RethApi<Provider> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

struct RethApiInner<Provider> {
    /// The provider that can interact with the chain.
    provider: Provider,
    /// The type that can spawn tasks which would otherwise block.
    task_spawner: Box<dyn TaskSpawner>,
}
