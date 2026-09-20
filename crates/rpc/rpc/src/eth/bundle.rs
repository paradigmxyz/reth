//! `Eth` bundle implementation and helpers.

use alloy_consensus::{transaction::TxHashRef, EnvKzgSettings, Transaction as _};
use alloy_eips::eip7840::BlobParams;
use alloy_evm::env::BlockEnvironment;
use alloy_primitives::{uint, Keccak256, U256};
use alloy_rpc_types_mev::{EthCallBundle, EthCallBundleResponse, EthCallBundleTransactionResult};
use jsonrpsee::core::RpcResult;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::{ConfigureEvm, Evm};
use reth_rpc_eth_api::{
    helpers::{Call, EthTransactions, LoadPendingBlock},
    EthCallBundleApiServer, FromEthApiError, FromEvmError,
};
use reth_rpc_eth_types::{
    utils::recover_raw_transaction, EthApiError, PendingBlockEnv, PendingBlockEnvOrigin,
    RpcInvalidTransactionError,
};
use reth_storage_api::BlockReaderIdExt;
use reth_tasks::pool::BlockingTaskGuard;
use reth_transaction_pool::{
    EthBlobTransactionSidecar, EthPoolTransaction, PoolPooledTx, PoolTransaction, TransactionPool,
};
use revm::{
    context::Block, context_interface::result::ResultAndState, DatabaseCommit, DatabaseRef,
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

        // Validate gas limit against the configured call gas limit before any DB calls
        let call_gas_limit = self.inner.eth_api.call_gas_limit();
        if let Some(gas_limit) = gas_limit &&
            gas_limit > call_gas_limit
        {
            return Err(
                EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh).into()
            )
        }

        let transactions = txs
            .into_iter()
            .map(|tx| recover_raw_transaction::<PoolPooledTx<Eth::Pool>>(&tx))
            .collect::<Result<Vec<_>, _>>()?;

        let block_id: alloy_rpc_types_eth::BlockId = state_block_number.into();
        // Note: the block number is considered the `parent` block: <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2104>
        let (mut evm_env, at, parent) = if block_id.is_pending() {
            let PendingBlockEnv { evm_env, origin } = self.eth_api().pending_block_env_and_cfg()?;
            let at = origin.state_block_id();
            let parent = match origin {
                PendingBlockEnvOrigin::ActualPending(block, _) => block.clone_sealed_header(),
                PendingBlockEnvOrigin::DerivedFromLatest(header) => header,
            };
            (evm_env, at, parent)
        } else {
            let parent = self
                .eth_api()
                .provider()
                .sealed_header_by_id(block_id)
                .map_err(Eth::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(block_id))?;
            let evm_env = self.eth_api().evm_env_for_header(&parent)?;
            (evm_env, parent.hash().into(), parent)
        };

        if let Some(coinbase) = coinbase {
            evm_env.block_env.inner_mut().beneficiary = coinbase;
        }

        // need to adjust the timestamp for the next block
        if let Some(timestamp) = timestamp {
            evm_env.block_env.inner_mut().timestamp = U256::from(timestamp);
        } else {
            evm_env.block_env.inner_mut().timestamp += uint!(12_U256);
        }

        if let Some(difficulty) = difficulty {
            evm_env.block_env.inner_mut().difficulty = U256::from(difficulty);
        }

        // Validate that the bundle does not contain more than MAX_BLOB_NUMBER_PER_BLOCK blob
        // transactions.
        let blob_gas_used = transactions.iter().filter_map(|tx| tx.blob_gas_used()).sum::<u64>();
        if blob_gas_used > 0 {
            let blob_params = self
                .eth_api()
                .provider()
                .chain_spec()
                .blob_params_at_timestamp(evm_env.block_env.timestamp().saturating_to())
                .unwrap_or_else(BlobParams::cancun);
            if blob_gas_used > blob_params.max_blob_gas_per_block() {
                return Err(EthApiError::InvalidParams(
                    EthBundleError::Eip4844BlobGasExceeded(blob_params.max_blob_gas_per_block())
                        .to_string(),
                )
                .into())
            }
        }

        // Apply gas limit: default to call gas limit unless user requests a smaller limit
        evm_env.block_env.inner_mut().gas_limit = gas_limit.unwrap_or(call_gas_limit);

        // The bundle is simulated on top of the state block, so default to the next block's base
        // fee. <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2130>
        if let Some(base_fee) = base_fee {
            evm_env.block_env.inner_mut().basefee = base_fee.try_into().unwrap_or(u64::MAX);
        } else if let Some(next_base_fee) = self
            .eth_api()
            .provider()
            .chain_spec()
            .next_block_base_fee(&parent, evm_env.block_env.timestamp().saturating_to())
        {
            evm_env.block_env.inner_mut().basefee = next_base_fee;
        }

        let state_block_number = evm_env.block_env.number();
        // use the block number of the request
        evm_env.block_env.inner_mut().number = U256::from(block_number);

        self.eth_api()
            .spawn_with_state_at_block(at, move |eth_api, db| {
                let coinbase = evm_env.block_env.beneficiary();
                let basefee = evm_env.block_env.basefee();

                let initial_coinbase = db
                    .basic_ref(coinbase)
                    .map_err(Eth::Error::from_eth_err)?
                    .map(|acc| acc.balance)
                    .unwrap_or_default();
                let mut coinbase_balance_before_tx = initial_coinbase;
                let mut coinbase_balance_after_tx = initial_coinbase;
                let mut total_gas_used = 0u64;
                let mut total_gas_fees = U256::ZERO;
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
                    let ResultAndState { result, state } = evm
                        .transact(eth_api.evm_config().tx_env(&tx))
                        .map_err(Eth::Error::from_evm_err)?;

                    let gas_price = tx
                        .effective_tip_per_gas(basefee)
                        .expect("fee is always valid; execution succeeded");
                    let gas_used = result.tx_gas_used();
                    total_gas_used += gas_used;

                    let gas_fees = U256::from(gas_used) * U256::from(gas_price);
                    total_gas_fees += gas_fees;

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
                let eth_sent_to_coinbase = coinbase_diff.saturating_sub(total_gas_fees);
                let bundle_gas_price =
                    coinbase_diff.checked_div(U256::from(total_gas_used)).unwrap_or_default();
                let res = EthCallBundleResponse {
                    bundle_gas_price,
                    bundle_hash: hasher.finalize(),
                    coinbase_diff,
                    eth_sent_to_coinbase,
                    gas_fees: total_gas_fees,
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

/// Container type for `EthBundle` internals
#[derive(Debug)]
struct EthBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    #[expect(dead_code)]
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
    #[error("blob gas usage exceeds the limit of {0} gas per block.")]
    Eip4844BlobGasExceeded(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EthApiBuilder;
    use alloy_consensus::{transaction::SignerRecoverable, TxEip1559};
    use alloy_eips::{eip1559::INITIAL_BASE_FEE, eip2718::Encodable2718, BlockNumberOrTag};
    use alloy_genesis::{Genesis, GenesisAccount};
    use alloy_primitives::{Address, TxKind};
    use reth_chain_state::ExecutedBlock;
    use reth_chainspec::ChainSpecBuilder;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::Block as EthBlock;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    };
    use reth_storage_overlay::OverlayManager;
    use reth_testing_utils::generators::{self, generate_key, sign_tx_with_key_pair};
    use reth_transaction_pool::test_utils::testing_pool;

    /// The bundle is simulated in the block after the state block, so a transaction that only
    /// covers the next block's base fee, but not the state block's, must be accepted.
    #[tokio::test(flavor = "multi_thread")]
    async fn call_bundle_uses_next_block_base_fee() {
        let mut rng = generators::rng();
        let key = generate_key(&mut rng);
        // genesis is empty, so the next base fee is 7/8 of the initial base fee
        let next_base_fee = INITIAL_BASE_FEE - INITIAL_BASE_FEE / 8;
        let tx = sign_tx_with_key_pair(
            key,
            reth_ethereum_primitives::Transaction::Eip1559(TxEip1559 {
                chain_id: 1,
                nonce: 0,
                gas_limit: 21_000,
                max_fee_per_gas: (next_base_fee + 1) as u128,
                max_priority_fee_per_gas: (next_base_fee + 1) as u128,
                to: TxKind::Call(Address::random()),
                ..Default::default()
            }),
        );
        let sender = tx.recover_signer().unwrap();

        let genesis = Genesis::default()
            .with_gas_limit(30_000_000)
            .with_base_fee(Some(INITIAL_BASE_FEE as u128))
            .extend_accounts([(
                sender,
                GenesisAccount::default().with_balance(U256::from(10u128.pow(18))),
            )]);
        let chain_spec =
            Arc::new(ChainSpecBuilder::mainnet().cancun_activated().genesis(genesis).build());
        let overlay_manager = OverlayManager::default();
        let factory = create_test_provider_factory_with_chain_spec(chain_spec)
            .with_overlay_manager(overlay_manager.clone());
        init_genesis(&factory).unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();

        let eth_api = EthApiBuilder::new(
            provider.clone(),
            testing_pool(),
            NoopNetwork::default(),
            EthEvmConfig::new(provider.chain_spec()),
        )
        .build();
        let api = EthBundle::new(eth_api, BlockingTaskGuard::new(4));

        let bundle = EthCallBundle {
            txs: vec![tx.encoded_2718().into()],
            block_number: 1,
            state_block_number: BlockNumberOrTag::Number(0),
            ..Default::default()
        };
        for state_block_number in
            [BlockNumberOrTag::Number(0), BlockNumberOrTag::Latest, BlockNumberOrTag::Pending]
        {
            for base_fee in [None, Some((next_base_fee - 1) as u128)] {
                let response = api
                    .call_bundle(EthCallBundle { state_block_number, base_fee, ..bundle.clone() })
                    .await
                    .unwrap();
                let expected_tip =
                    (next_base_fee + 1) as u128 - base_fee.unwrap_or(next_base_fee as u128);
                assert_bundle_fees(&response, expected_tip);
            }
        }

        // An actual pending block is not canonical and has its own base fee.
        let chain_spec = provider.chain_spec();
        let mut header = chain_spec.genesis_header().clone();
        header.parent_hash = chain_spec.genesis_hash();
        header.number = 1;
        header.timestamp = 12;
        header.base_fee_per_gas = Some(next_base_fee);
        let pending_block = ExecutedBlock {
            recovered_block: Arc::new(RecoveredBlock::new_unhashed(
                EthBlock { header, body: Default::default() },
                vec![],
            )),
            ..Default::default()
        };
        overlay_manager.insert_block(pending_block.clone());
        provider.canonical_in_memory_state().set_pending_block(pending_block);
        let pending_next_base_fee = next_base_fee - next_base_fee / 8;
        for base_fee in [None, Some((pending_next_base_fee - 1) as u128)] {
            let response = api
                .call_bundle(EthCallBundle {
                    block_number: 2,
                    state_block_number: BlockNumberOrTag::Pending,
                    base_fee,
                    ..bundle.clone()
                })
                .await
                .unwrap();
            let expected_tip =
                (next_base_fee + 1) as u128 - base_fee.unwrap_or(pending_next_base_fee as u128);
            assert_bundle_fees(&response, expected_tip);
        }
    }

    fn assert_bundle_fees(response: &EthCallBundleResponse, expected_tip: u128) {
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.total_gas_used, 21_000);
        let gas_price = U256::from(expected_tip);
        let gas_fees = gas_price * U256::from(21_000);
        assert_eq!(response.results[0].gas_price, gas_price);
        assert_eq!(response.results[0].gas_fees, gas_fees);
        assert_eq!(response.gas_fees, gas_fees);
        assert_eq!(response.bundle_gas_price, gas_price);
        assert_eq!(response.coinbase_diff, gas_fees);
    }
}
