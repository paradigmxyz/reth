//! `Eth` bundle implementation and helpers.

use alloy_consensus::{EnvKzgSettings, Transaction as _};
use alloy_eips::eip4844::MAX_DATA_GAS_PER_BLOCK;
use alloy_primitives::{Keccak256, U256};
use alloy_rpc_types_mev::{EthCallBundle, EthCallBundleResponse, EthCallBundleTransactionResult};
use jsonrpsee::core::RpcResult;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv, Evm};
use reth_primitives_traits::SignedTransaction;
use reth_revm::database::StateProviderDatabase;
use reth_rpc_eth_api::{
    helpers::{Call, EthTransactions, LoadPendingBlock},
    EthCallBundleApiServer, FromEthApiError, FromEvmError,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError, RpcInvalidTransactionError};
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, PoolPooledTx, PoolTransaction, TransactionPool,
};
use revm::{
    db::{CacheDB, DatabaseCommit, DatabaseRef},
    primitives::ResultAndState,
};
use std::sync::Arc;

/// `Eth` bundle implementation.
pub struct EthBundle<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthBundleInner<Eth>>,
}

impl<Eth> EthBundle<Eth> {
    /// Create a new `EthBundle` instance.
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        Self { inner: Arc::new(EthBundleInner { eth_api, blocking_task_guard }) }
    }

    /// Access the underlying `Eth` API.
    pub fn eth_api(&self) -> &Eth {
        &self.inner.eth_api
    }
}

impl<Eth> EthBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    /// Simulates a bundle of transactions at the top of a given block number with the state of
    /// another (or the same) block. This can be used to simulate future blocks with the current
    /// state, or it can be used to simulate a past block. The sender is responsible for signing the
    /// transactions and using the correct nonce and ensuring validity
    pub async fn call_bundle(
        &self,
        bundle: EthCallBundle,
    ) -> Result<EthCallBundleResponse, Eth::Error> {
        let EthCallBundle {
            txs,
            block_number,
            coinbase,
            state_block_number,
            timeout: _,
            timestamp,
            gas_limit,
            difficulty,
            base_fee,
            ..
        } = bundle;
        if txs.is_empty() {
            return Err(EthApiError::InvalidParams(
                EthBundleError::EmptyBundleTransactions.to_string(),
            )
            .into())
        }
        if block_number == 0 {
            return Err(EthApiError::InvalidParams(
                EthBundleError::BundleMissingBlockNumber.to_string(),
            )
            .into())
        }

        let transactions = txs
            .into_iter()
            .map(|tx| recover_raw_transaction::<PoolPooledTx<Eth::Pool>>(&tx))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Vec<_>>();

        // Validate that the bundle does not contain more than MAX_BLOB_NUMBER_PER_BLOCK blob
        // transactions.
        if transactions.iter().filter_map(|tx| tx.blob_gas_used()).sum::<u64>() >
            MAX_DATA_GAS_PER_BLOCK
        {
            return Err(EthApiError::InvalidParams(
                EthBundleError::Eip4844BlobGasExceeded.to_string(),
            )
            .into())
        }

        let block_id: alloy_rpc_types_eth::BlockId = state_block_number.into();
        // Note: the block number is considered the `parent` block: <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2104>
        let (mut evm_env, at) = self.eth_api().evm_env_at(block_id).await?;

        if let Some(coinbase) = coinbase {
            evm_env.block_env.coinbase = coinbase;
        }

        // need to adjust the timestamp for the next block
        if let Some(timestamp) = timestamp {
            evm_env.block_env.timestamp = U256::from(timestamp);
        } else {
            evm_env.block_env.timestamp += U256::from(12);
        }

        if let Some(difficulty) = difficulty {
            evm_env.block_env.difficulty = U256::from(difficulty);
        }

        // default to call gas limit unless user requests a smaller limit
        evm_env.block_env.gas_limit = U256::from(self.inner.eth_api.call_gas_limit());
        if let Some(gas_limit) = gas_limit {
            let gas_limit = U256::from(gas_limit);
            if gas_limit > evm_env.block_env.gas_limit {
                return Err(
                    EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh).into()
                )
            }
            evm_env.block_env.gas_limit = gas_limit;
        }

        if let Some(base_fee) = base_fee {
            evm_env.block_env.basefee = U256::from(base_fee);
        }

        let state_block_number = evm_env.block_env.number;
        // use the block number of the request
        evm_env.block_env.number = U256::from(block_number);

        let eth_api = self.eth_api().clone();

        self.eth_api()
            .spawn_with_state_at_block(at, move |state| {
                let coinbase = evm_env.block_env.coinbase;
                let basefee = Some(evm_env.block_env.basefee.to::<u64>());
                let db = CacheDB::new(StateProviderDatabase::new(state));

                let initial_coinbase = db
                    .basic_ref(coinbase)
                    .map_err(Eth::Error::from_eth_err)?
                    .map(|acc| acc.balance)
                    .unwrap_or_default();
                let mut coinbase_balance_before_tx = initial_coinbase;
                let mut coinbase_balance_after_tx = initial_coinbase;
                let mut total_gas_used = 0u64;
                let mut total_gas_fess = U256::ZERO;
                let mut hasher = Keccak256::new();

                let mut evm = eth_api.evm_config().evm_with_env(db, evm_env);

                let mut results = Vec::with_capacity(transactions.len());
                let mut transactions = transactions.into_iter().peekable();

                while let Some(tx) = transactions.next() {
                    let signer = tx.signer();
                    let tx = {
                        let mut tx = <Eth::Pool as TransactionPool>::Transaction::from_pooled(tx);

                        if let EthBlobTransactionSidecar::Present(sidecar) = tx.take_blob() {
                            tx.validate_blob(&sidecar, EnvKzgSettings::Default.get()).map_err(
                                |e| {
                                    Eth::Error::from_eth_err(EthApiError::InvalidParams(
                                        e.to_string(),
                                    ))
                                },
                            )?;
                        }

                        tx.into_consensus()
                    };

                    hasher.update(*tx.tx_hash());
                    let gas_price = tx.effective_gas_price(basefee);
                    let ResultAndState { result, state } = evm
                        .transact(eth_api.evm_config().tx_env(&tx, signer))
                        .map_err(Eth::Error::from_evm_err)?;

                    let gas_used = result.gas_used();
                    total_gas_used += gas_used;

                    let gas_fees = U256::from(gas_used) * U256::from(gas_price);
                    total_gas_fess += gas_fees;

                    // coinbase is always present in the result state
                    coinbase_balance_after_tx =
                        state.get(&coinbase).map(|acc| acc.info.balance).unwrap_or_default();
                    let coinbase_diff =
                        coinbase_balance_after_tx.saturating_sub(coinbase_balance_before_tx);
                    let eth_sent_to_coinbase = coinbase_diff.saturating_sub(gas_fees);

                    // update the coinbase balance
                    coinbase_balance_before_tx = coinbase_balance_after_tx;

                    // set the return data for the response
                    let (value, revert) = if result.is_success() {
                        let value = result.into_output().unwrap_or_default();
                        (Some(value), None)
                    } else {
                        let revert = result.into_output().unwrap_or_default();
                        (None, Some(revert))
                    };

                    let tx_res = EthCallBundleTransactionResult {
                        coinbase_diff,
                        eth_sent_to_coinbase,
                        from_address: signer,
                        gas_fees,
                        gas_price: U256::from(gas_price),
                        gas_used,
                        to_address: tx.to(),
                        tx_hash: *tx.tx_hash(),
                        value,
                        revert,
                    };
                    results.push(tx_res);

                    // need to apply the state changes of this call before executing the
                    // next call
                    if transactions.peek().is_some() {
                        // need to apply the state changes of this call before executing
                        // the next call
                        evm.db_mut().commit(state)
                    }
                }

                // populate the response

                let coinbase_diff = coinbase_balance_after_tx.saturating_sub(initial_coinbase);
                let eth_sent_to_coinbase = coinbase_diff.saturating_sub(total_gas_fess);
                let bundle_gas_price =
                    coinbase_diff.checked_div(U256::from(total_gas_used)).unwrap_or_default();
                let res = EthCallBundleResponse {
                    bundle_gas_price,
                    bundle_hash: hasher.finalize(),
                    coinbase_diff,
                    eth_sent_to_coinbase,
                    gas_fees: total_gas_fess,
                    results,
                    state_block_number: state_block_number.to(),
                    total_gas_used,
                };

                Ok(res)
            })
            .await
    }
}

#[async_trait::async_trait]
impl<Eth> EthCallBundleApiServer for EthBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    async fn call_bundle(&self, request: EthCallBundle) -> RpcResult<EthCallBundleResponse> {
        Self::call_bundle(self, request).await.map_err(Into::into)
    }
}

/// Container type for  `EthBundle` internals
#[derive(Debug)]
struct EthBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    #[allow(dead_code)]
    blocking_task_guard: BlockingTaskGuard,
}

impl<Eth> std::fmt::Debug for EthBundle<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthBundle").finish_non_exhaustive()
    }
}

impl<Eth> Clone for EthBundle<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

/// [`EthBundle`] specific errors.
#[derive(Debug, thiserror::Error)]
pub enum EthBundleError {
    /// Thrown if the bundle does not contain any transactions.
    #[error("bundle missing txs")]
    EmptyBundleTransactions,
    /// Thrown if the bundle does not contain a block number, or block number is 0.
    #[error("bundle missing blockNumber")]
    BundleMissingBlockNumber,
    /// Thrown when the blob gas usage of the blob transactions in a bundle exceed the maximum.
    #[error("blob gas usage exceeds the limit of {MAX_DATA_GAS_PER_BLOCK} gas per block.")]
    Eip4844BlobGasExceeded,
}
