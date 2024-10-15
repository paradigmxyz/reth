//! `Eth` bundle implementation and helpers.

use std::sync::Arc;

use alloy_primitives::{Keccak256, U256};
use alloy_rpc_types_mev::{EthCallBundle, EthCallBundleResponse, EthCallBundleTransactionResult};
use jsonrpsee::core::RpcResult;
use reth_chainspec::EthChainSpec;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    revm_primitives::db::{DatabaseCommit, DatabaseRef},
    PooledTransactionsElement,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_eth_api::{FromEthApiError, FromEvmError};
use reth_tasks::pool::BlockingTaskGuard;
use revm::{
    db::CacheDB,
    primitives::{ResultAndState, TxEnv},
};
use revm_primitives::{EnvKzgSettings, EnvWithHandlerCfg, SpecId, MAX_BLOB_GAS_PER_BLOCK};

use reth_provider::{ChainSpecProvider, HeaderProvider};
use reth_rpc_eth_api::{
    helpers::{Call, EthTransactions, LoadPendingBlock},
    EthCallBundleApiServer,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError, RpcInvalidTransactionError};
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
            state_block_number,
            timestamp,
            gas_limit,
            difficulty,
            base_fee,
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
            .map(recover_raw_transaction)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|tx| tx.into_components())
            .collect::<Vec<_>>();

        // Validate that the bundle does not contain more than MAX_BLOB_NUMBER_PER_BLOCK blob
        // transactions.
        if transactions
            .iter()
            .filter_map(|(tx, _)| {
                if let PooledTransactionsElement::BlobTransaction(tx) = tx {
                    Some(tx.transaction.tx.blob_gas())
                } else {
                    None
                }
            })
            .sum::<u64>() >
            MAX_BLOB_GAS_PER_BLOCK
        {
            return Err(EthApiError::InvalidParams(
                EthBundleError::Eip4844BlobGasExceeded.to_string(),
            )
            .into())
        }

        let block_id: alloy_rpc_types::BlockId = state_block_number.into();
        // Note: the block number is considered the `parent` block: <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2104>
        let (cfg, mut block_env, at) = self.eth_api().evm_env_at(block_id).await?;

        // need to adjust the timestamp for the next block
        if let Some(timestamp) = timestamp {
            block_env.timestamp = U256::from(timestamp);
        } else {
            block_env.timestamp += U256::from(12);
        }

        if let Some(difficulty) = difficulty {
            block_env.difficulty = U256::from(difficulty);
        }

        if let Some(gas_limit) = gas_limit {
            block_env.gas_limit = U256::from(gas_limit);
        }

        if let Some(base_fee) = base_fee {
            block_env.basefee = U256::from(base_fee);
        } else if cfg.handler_cfg.spec_id.is_enabled_in(SpecId::LONDON) {
            let parent_block = block_env.number.saturating_to::<u64>();
            // here we need to fetch the _next_ block's basefee based on the parent block <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2130>
            let parent = LoadPendingBlock::provider(self.eth_api())
                .header_by_number(parent_block)
                .map_err(Eth::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(parent_block.into()))?;
            if let Some(base_fee) = parent.next_block_base_fee(
                LoadPendingBlock::provider(self.eth_api())
                    .chain_spec()
                    .base_fee_params_at_block(parent_block),
            ) {
                block_env.basefee = U256::from(base_fee);
            }
        }

        let state_block_number = block_env.number;
        // use the block number of the request
        block_env.number = U256::from(block_number);

        let eth_api = self.eth_api().clone();

        self.eth_api()
            .spawn_with_state_at_block(at, move |state| {
                let coinbase = block_env.coinbase;
                let basefee = Some(block_env.basefee.to::<u64>());
                let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, TxEnv::default());
                let db = CacheDB::new(StateProviderDatabase::new(state));

                let initial_coinbase = DatabaseRef::basic_ref(&db, coinbase)
                    .map_err(Eth::Error::from_eth_err)?
                    .map(|acc| acc.balance)
                    .unwrap_or_default();
                let mut coinbase_balance_before_tx = initial_coinbase;
                let mut coinbase_balance_after_tx = initial_coinbase;
                let mut total_gas_used = 0u64;
                let mut total_gas_fess = U256::ZERO;
                let mut hasher = Keccak256::new();

                let mut evm = Call::evm_config(&eth_api).evm_with_env(db, env);

                let mut results = Vec::with_capacity(transactions.len());
                let mut transactions = transactions.into_iter().peekable();

                while let Some((tx, signer)) = transactions.next() {
                    // Verify that the given blob data, commitments, and proofs are all valid for
                    // this transaction.
                    if let PooledTransactionsElement::BlobTransaction(ref tx) = tx {
                        tx.validate(EnvKzgSettings::Default.get()).map_err(|e| {
                            Eth::Error::from_eth_err(EthApiError::InvalidParams(e.to_string()))
                        })?;
                    }

                    let tx = tx.into_transaction();

                    hasher.update(tx.hash());
                    let gas_price = tx
                        .effective_tip_per_gas(basefee)
                        .ok_or_else(|| RpcInvalidTransactionError::FeeCapTooLow)
                        .map_err(Eth::Error::from_eth_err)?;
                    Call::evm_config(&eth_api).fill_tx_env(evm.tx_mut(), &tx, signer);
                    let ResultAndState { result, state } =
                        evm.transact().map_err(Eth::Error::from_evm_err)?;

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
    /// Thrown when the blob gas usage of the blob transactions in a bundle exceed
    /// [`MAX_BLOB_GAS_PER_BLOCK`].
    #[error("blob gas usage exceeds the limit of {MAX_BLOB_GAS_PER_BLOCK} gas per block.")]
    Eip4844BlobGasExceeded,
}
