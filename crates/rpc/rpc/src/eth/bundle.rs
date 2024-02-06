//! `Eth` bundle implementation and helpers.

use crate::{
    eth::{
        error::{EthApiError, EthResult, RpcInvalidTransactionError},
        revm_utils::FillableTransaction,
        utils::recover_raw_transaction,
        EthTransactions,
    },
    BlockingTaskGuard,
};
use jsonrpsee::core::RpcResult;
use reth_primitives::{
    keccak256,
    revm_primitives::db::{DatabaseCommit, DatabaseRef},
    U256,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_api::EthCallBundleApiServer;
use reth_rpc_types::{EthCallBundle, EthCallBundleResponse, EthCallBundleTransactionResult};
use revm::{
    db::CacheDB,
    primitives::{ResultAndState, TxEnv},
};
use revm_primitives::EnvWithHandlerCfg;
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
}

impl<Eth> EthBundle<Eth>
where
    Eth: EthTransactions + 'static,
{
    /// Simulates a bundle of transactions at the top of a given block number with the state of
    /// another (or the same) block. This can be used to simulate future blocks with the current
    /// state, or it can be used to simulate a past block. The sender is responsible for signing the
    /// transactions and using the correct nonce and ensuring validity
    pub async fn call_bundle(&self, bundle: EthCallBundle) -> EthResult<EthCallBundleResponse> {
        let EthCallBundle { txs, block_number, state_block_number, timestamp } = bundle;
        if txs.is_empty() {
            return Err(EthApiError::InvalidParams(
                EthBundleError::EmptyBundleTransactions.to_string(),
            ))
        }
        if block_number.to::<u64>() == 0 {
            return Err(EthApiError::InvalidParams(
                EthBundleError::BundleMissingBlockNumber.to_string(),
            ))
        }

        let transactions =
            txs.into_iter().map(recover_raw_transaction).collect::<Result<Vec<_>, _>>()?;
        let block_id: reth_rpc_types::BlockId = state_block_number.into();
        let (cfg, mut block_env, at) = self.inner.eth_api.evm_env_at(block_id).await?;

        // need to adjust the timestamp for the next block
        if let Some(timestamp) = timestamp {
            block_env.timestamp = U256::from(timestamp);
        } else {
            block_env.timestamp += U256::from(12);
        }

        let state_block_number = block_env.number;
        // use the block number of the request
        block_env.number = U256::from(block_number);

        self.inner
            .eth_api
            .spawn_with_state_at_block(at, move |state| {
                let coinbase = block_env.coinbase;
                let basefee = Some(block_env.basefee.to::<u64>());
                let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, TxEnv::default());
                let db = CacheDB::new(StateProviderDatabase::new(state));

                let initial_coinbase = DatabaseRef::basic_ref(&db, coinbase)?
                    .map(|acc| acc.balance)
                    .unwrap_or_default();
                let mut coinbase_balance_before_tx = initial_coinbase;
                let mut coinbase_balance_after_tx = initial_coinbase;
                let mut total_gas_used = 0u64;
                let mut total_gas_fess = U256::ZERO;
                let mut hash_bytes = Vec::with_capacity(32 * transactions.len());

                let mut evm =
                    revm::Evm::builder().with_db(db).with_env_with_handler_cfg(env).build();

                let mut results = Vec::with_capacity(transactions.len());
                let mut transactions = transactions.into_iter().peekable();

                while let Some(tx) = transactions.next() {
                    let tx = tx.into_ecrecovered_transaction();
                    hash_bytes.extend_from_slice(tx.hash().as_slice());
                    let gas_price = tx
                        .effective_tip_per_gas(basefee)
                        .ok_or_else(|| RpcInvalidTransactionError::FeeCapTooLow)?;
                    tx.try_fill_tx_env(evm.tx_mut())?;
                    let ResultAndState { result, state } = evm.transact()?;

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
                        from_address: tx.signer(),
                        gas_fees,
                        gas_price: U256::from(gas_price),
                        gas_used,
                        to_address: tx.to(),
                        tx_hash: tx.hash(),
                        value,
                        revert,
                    };
                    results.push(tx_res);

                    // need to apply the state changes of this call before executing the
                    // next call
                    if transactions.peek().is_some() {
                        // need to apply the state changes of this call before executing
                        // the next call
                        evm.context.evm.db.commit(state)
                    }
                }

                // populate the response

                let coinbase_diff = coinbase_balance_after_tx.saturating_sub(initial_coinbase);
                let eth_sent_to_coinbase = coinbase_diff.saturating_sub(total_gas_fess);
                let bundle_gas_price =
                    coinbase_diff.checked_div(U256::from(total_gas_used)).unwrap_or_default();
                let res = EthCallBundleResponse {
                    bundle_gas_price,
                    bundle_hash: keccak256(&hash_bytes),
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
    Eth: EthTransactions + 'static,
{
    async fn call_bundle(&self, request: EthCallBundle) -> RpcResult<EthCallBundleResponse> {
        Ok(EthBundle::call_bundle(self, request).await?)
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

/// [EthBundle] specific errors.
#[derive(Debug, thiserror::Error)]
pub enum EthBundleError {
    /// Thrown if the bundle does not contain any transactions.
    #[error("bundle missing txs")]
    EmptyBundleTransactions,
    /// Thrown if the bundle does not contain a block number, or block number is 0.
    #[error("bundle missing blockNumber")]
    BundleMissingBlockNumber,
}
