//! `Eth` Sim bundle implementation and helpers.

use std::{sync::Arc, time::Duration};

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::U256;
use alloy_rpc_types::{calc_excess_blob_gas, BlockId};
use alloy_rpc_types_mev::{
    BundleItem, SendBundleRequest, SimBundleLogs, SimBundleOverrides, SimBundleResponse, Validity,
};
use jsonrpsee::core::RpcResult;
use reth_chainspec::EthChainSpec;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::revm_primitives::db::{DatabaseCommit, DatabaseRef};
use reth_provider::{ChainSpecProvider, HeaderProvider};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_api::MevSimApiServer;
use reth_rpc_eth_api::{
    helpers::{Call, EthTransactions, LoadPendingBlock},
    FromEthApiError,
};
use reth_rpc_eth_types::{utils::recover_raw_transaction, EthApiError};
use reth_tasks::pool::BlockingTaskGuard;
use revm::{
    db::CacheDB,
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ResultAndState, SpecId, TxEnv,
    },
};
use tracing::info;

/// Maximum depth for bundle simulation
const MAX_DEPTH: u32 = 5;

/// Maximum body size for bundle simulation
const MAX_BODY_SIZE: usize = 50;

/// Default simulation timeout
const DEFAULT_SIM_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum simulation timeout
const MAX_SIM_TIMEOUT: Duration = Duration::from_secs(30);

/// `Eth` sim bundle implementation.
pub struct EthSimBundle<Eth> {
    /// All nested fields bundled together.
    inner: Arc<EthSimBundleInner<Eth>>,
}

impl<Eth> EthSimBundle<Eth> {
    /// Create a new `EthSimBundle` instance.
    pub fn new(eth_api: Eth, blocking_task_guard: BlockingTaskGuard) -> Self {
        Self { inner: Arc::new(EthSimBundleInner { eth_api, blocking_task_guard }) }
    }
}

impl<Eth> EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    async fn sim_bundle_recursive(
        &self,
        request: SendBundleRequest,
        block_env: BlockEnv,
        cfg: CfgEnvWithHandlerCfg,
        logs: bool,
    ) -> Result<SimBundleResponse, Eth::Error> {
        let SendBundleRequest { bundle_body, protocol_version, inclusion, validity, privacy } =
            request;

        let current_block = block_env.number.saturating_to::<u64>();

        if current_block < inclusion.block || current_block > inclusion.max_block.unwrap_or(0) {
            todo!("return error")
        }

        let mut total_gas_used = 0;
        let body_logs: Vec<SimBundleLogs> = Vec::new();
        let mut total_profit = U256::ZERO;

        // Extract constraints into convinient format
        let mut refund_idx = vec![false; bundle_body.len()];
        let mut refund_percents = vec![0; bundle_body.len()];

        let Validity { refund, .. } = validity.unwrap();

        for el in refund.unwrap() {
            refund_idx[el.body_idx as usize] = true;
            refund_percents[el.body_idx as usize] = el.percent;
        }

        let mut sim_result: Option<SimBundleResponse> = None;

        for (i, el) in bundle_body.iter().enumerate() {
            let el = el.clone();
            let mut body_logs = body_logs.clone();
            let mut coinbase_diff = U256::ZERO;
            sim_result = Some(match el {
                BundleItem::Tx { tx, can_revert } => {
                    let eth_api = self.inner.eth_api.clone();
                    let block_env_closure = block_env.clone();
                    let cfg_closure = cfg.clone();
                    let recovered_tx = recover_raw_transaction(tx.clone()).unwrap();
                    let pooled_tx = recovered_tx.into_transaction();
                    let tx = pooled_tx.into_transaction();
                    let sim_result = self
                        .inner
                        .eth_api
                        .spawn_with_state_at_block(
                            BlockId::Number(current_block.into()),
                            move |state| {
                                let coinbase = block_env_closure.coinbase;
                                let env = EnvWithHandlerCfg::new_with_cfg_env(
                                    cfg_closure,
                                    block_env_closure,
                                    TxEnv::default(),
                                );
                                let db = CacheDB::new(StateProviderDatabase::new(state));

                                let initial_coinbase = DatabaseRef::basic_ref(&db, coinbase)
                                    .map_err(Eth::Error::from_eth_err)?
                                    .map(|acc| acc.balance)
                                    .unwrap_or_default();
                                info!("initial_coinbase: {:?}", initial_coinbase);
                                let mut coinbase_balance_before_tx = initial_coinbase;
                                let mut coinbase_balance_after_tx = initial_coinbase;

                                let mut evm = Call::evm_config(&eth_api).evm_with_env(db, env);

                                Call::evm_config(&eth_api).fill_tx_env(evm.tx_mut(), &tx, coinbase);
                                let ResultAndState { result, state } =
                                    evm.transact().map_err(Eth::Error::from_eth_err)?;

                                if !result.is_success() && !can_revert {
                                    return Ok(SimBundleResponse {
                                        success: false,
                                        state_block: current_block,
                                        error: Some("Transaction reverted".to_string()),
                                        logs: Some(body_logs),
                                        gas_used: total_gas_used,
                                        mev_gas_price: U256::ZERO,
                                        profit: U256::ZERO,
                                        refundable_value: U256::ZERO,
                                    });
                                }

                                let gas_used = result.gas_used();
                                total_gas_used += gas_used;

                                if logs {
                                    let sim_bundle_logs = SimBundleLogs {
                                        tx_logs: Some(result.logs().to_vec()),
                                        bundle_logs: None,
                                    };
                                    body_logs.push(sim_bundle_logs);
                                }

                                // coinbase is always present in the result state
                                let coinbase_balance_after_tx = state
                                    .get(&coinbase)
                                    .map(|acc| acc.info.balance)
                                    .unwrap_or_default();
                                coinbase_diff = coinbase_balance_after_tx
                                    .saturating_sub(coinbase_balance_before_tx);

                                total_profit += coinbase_diff;

                                evm.context.evm.db.commit(state);

                                let sim_result = SimBundleResponse {
                                    success: true,
                                    state_block: current_block,
                                    error: None,
                                    logs: Some(body_logs),
                                    gas_used: total_gas_used,
                                    mev_gas_price: U256::ZERO,
                                    profit: total_profit,
                                    refundable_value: U256::ZERO,
                                };
                                Ok(sim_result)
                            },
                        )
                        .await?;
                    sim_result
                }
                BundleItem::Hash { hash } => {
                    info!("Skipping hash-only bundle item: {:?}", hash);
                    return Err(Eth::Error::from_eth_err(EthApiError::InvalidBundle));
                }
                BundleItem::Bundle { bundle } => {
                    info!("Applying bundle: {:?}", bundle);
                    let inner_res = self
                        .sim_bundle_recursive(bundle.clone(), block_env.clone(), cfg.clone(), logs)
                        .await?;

                    total_gas_used += inner_res.gas_used;
                    if logs {
                        let inner_logs = SimBundleLogs {
                            tx_logs: None,
                            bundle_logs: Some(inner_res.logs.expect("logs should be Some")),
                        };
                        body_logs.push(inner_logs);
                    }
                    let sim_result = SimBundleResponse {
                        success: inner_res.success,
                        state_block: inner_res.state_block,
                        error: inner_res.error,
                        logs: Some(body_logs),
                        gas_used: total_gas_used,
                        mev_gas_price: inner_res.mev_gas_price,
                        profit: total_profit,
                        refundable_value: inner_res.refundable_value,
                    };
                    sim_result
                }
            });
            info!("sim_result: {:?}", sim_result);
            if let Some(ref mut result) = sim_result {
                // implement refund logic here
                if !refund_idx[i] {
                    result.refundable_value += coinbase_diff;
                }
            }
        }

        sim_result.ok_or(Eth::Error::from_eth_err(EthApiError::InvalidBundle))
    }

    /// Simulates a bundle of transactions.
    pub async fn sim_bundle(
        &self,
        request: SendBundleRequest,
        overrides: SimBundleOverrides,
    ) -> Result<SimBundleResponse, Eth::Error> {
        info!("mev_simBundle called, request: {:?}, overrides: {:?}", request, overrides);

        let SimBundleOverrides {
            parent_block,
            block_number,
            coinbase,
            timestamp,
            gas_limit,
            base_fee,
            timeout,
        } = overrides;

        let timeout =
            timeout.map(Duration::from_millis).unwrap_or(DEFAULT_SIM_TIMEOUT).min(MAX_SIM_TIMEOUT);

        // TODO: implement timeout

        let block_id = parent_block.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));

        let (cfg, mut block_env, at) = self.inner.eth_api.evm_env_at(block_id).await?;

        // apply overrides
        if let Some(block_number) = block_number {
            block_env.number = U256::from(block_number);
        }

        if let Some(coinbase) = coinbase {
            block_env.coinbase = coinbase;
        }

        if let Some(timestamp) = timestamp {
            block_env.timestamp = U256::from(timestamp);
        }

        if let Some(gas_limit) = gas_limit {
            block_env.gas_limit = U256::from(gas_limit);
        }

        if let Some(base_fee) = base_fee {
            block_env.basefee = U256::from(base_fee);
        } else if cfg.handler_cfg.spec_id.is_enabled_in(SpecId::LONDON) {
            let parent_block = block_env.number.saturating_to::<u64>();
            // here we need to fetch the _next_ block's basefee based on the parent block <https://github.com/flashbots/mev-geth/blob/fddf97beec5877483f879a77b7dea2e58a58d653/internal/ethapi/api.go#L2130>
            let parent = LoadPendingBlock::provider(&self.inner.eth_api)
                .header_by_number(parent_block)
                .map_err(Eth::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(parent_block.into()))?;
            if let Some(base_fee) = parent.next_block_base_fee(
                LoadPendingBlock::provider(&self.inner.eth_api)
                    .chain_spec()
                    .base_fee_params_at_block(parent_block),
            ) {
                block_env.basefee = U256::from(base_fee);
            }
        }

        if cfg.handler_cfg.spec_id.is_enabled_in(SpecId::CANCUN) {
            // Get the parent header
            let parent = LoadPendingBlock::provider(&self.inner.eth_api)
                .header_by_number(block_env.number.saturating_to::<u64>())
                .map_err(Eth::Error::from_eth_err)?
                .ok_or_else(|| {
                    EthApiError::HeaderNotFound((block_env.number.saturating_to::<u64>()).into())
                })?;

            // Calculate excess blob gas
            let excess_blob_gas = calc_excess_blob_gas(
                parent.excess_blob_gas.unwrap_or_default().try_into().unwrap(),
                parent.blob_gas_used.unwrap_or_default().try_into().unwrap(),
            );

            // Set blob excess gas and price
            block_env.set_blob_excess_gas_and_price(excess_blob_gas);
        }

        let bundle_res = self.sim_bundle_recursive(request, block_env, cfg, true);

        bundle_res.await
    }
}

#[async_trait::async_trait]
impl<Eth> MevSimApiServer for EthSimBundle<Eth>
where
    Eth: EthTransactions + LoadPendingBlock + Call + 'static,
{
    async fn sim_bundle(
        &self,
        request: SendBundleRequest,
        overrides: SimBundleOverrides,
    ) -> RpcResult<SimBundleResponse> {
        Self::sim_bundle(self, request, overrides).await.map_err(Into::into)
    }
}

/// Container type for `EthSimBundle` internals
#[derive(Debug)]
struct EthSimBundleInner<Eth> {
    /// Access to commonly used code of the `eth` namespace
    #[allow(dead_code)]
    eth_api: Eth,
    // restrict the number of concurrent tracing calls.
    #[allow(dead_code)]
    blocking_task_guard: BlockingTaskGuard,
}

impl<Eth> std::fmt::Debug for EthSimBundle<Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthSimBundle").finish_non_exhaustive()
    }
}

impl<Eth> Clone for EthSimBundle<Eth> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}
