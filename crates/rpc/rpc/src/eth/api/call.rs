//! Contains RPC handler implementations specific to endpoints that call/execute within evm.

use crate::{
    eth::{
        error::{ensure_success, EthApiError, EthResult, RevertError, RpcInvalidTransactionError},
        revm_utils::{
            apply_state_overrides, build_call_evm_env, caller_gas_allowance,
            cap_tx_gas_limit_with_caller_allowance, get_precompiles, inspect, prepare_call_env,
            transact, EvmOverrides,
        },
        EthTransactions,
    },
    EthApi,
};
use reth_network_api::NetworkInfo;
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{revm::env::tx_env_with_recovered, BlockId, BlockNumberOrTag, Bytes, U256};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProvider, StateProviderFactory,
};
use reth_revm::{access_list::AccessListInspector, database::StateProviderDatabase};
use reth_rpc_types::{
    state::StateOverride, AccessListWithGasUsed, Bundle, EthCallResponse, StateContext,
    TransactionRequest,
};
use reth_transaction_pool::TransactionPool;
use revm::{
    db::{CacheDB, DatabaseRef},
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, HaltReason, TransactTo,
    },
    DatabaseCommit,
};
use tracing::trace;

// Gas per transaction not creating a contract.
const MIN_TRANSACTION_GAS: u64 = 21_000u64;
const MIN_CREATE_GAS: u64 = 53_000u64;

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    /// Estimate gas needed for execution of the `request` at the [BlockId].
    pub async fn estimate_gas_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> EthResult<U256> {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;

        self.on_blocking_task(|this| async move {
            let state = this.state_at(at)?;
            this.estimate_gas_with(cfg, block_env, request, state, state_override)
        })
        .await
    }

    /// Executes the call request (`eth_call`) and returns the output
    pub async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> EthResult<Bytes> {
        let (res, _env) = self
            .transact_call_at(
                request,
                block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest)),
                overrides,
            )
            .await?;

        ensure_success(res.result)
    }

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides
    pub async fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        mut state_override: Option<StateOverride>,
    ) -> EthResult<Vec<EthCallResponse>> {
        let Bundle { transactions, block_override } = bundle;
        if transactions.is_empty() {
            return Err(EthApiError::InvalidParams(String::from("transactions are empty.")))
        }

        let StateContext { transaction_index, block_number } = state_context.unwrap_or_default();
        let transaction_index = transaction_index.unwrap_or_default();

        let target_block = block_number.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));

        let ((cfg, block_env, _), block) = futures::try_join!(
            self.evm_env_at(target_block),
            self.block_with_senders(target_block)
        )?;

        let Some(block) = block else { return Err(EthApiError::UnknownBlockNumber) };
        let gas_limit = self.inner.gas_cap;

        // we're essentially replaying the transactions in the block here, hence we need the state
        // that points to the beginning of the block, which is the state at the parent block
        let mut at = block.parent_hash;
        let mut replay_block_txs = true;

        // but if all transactions are to be replayed, we can use the state at the block itself
        let num_txs = transaction_index.index().unwrap_or(block.body.len());
        if num_txs == block.body.len() {
            at = block.hash();
            replay_block_txs = false;
        }

        self.spawn_with_state_at_block(at.into(), move |state| {
            let mut results = Vec::with_capacity(transactions.len());
            let mut db = CacheDB::new(StateProviderDatabase::new(state));

            if replay_block_txs {
                // only need to replay the transactions in the block if not all transactions are
                // to be replayed
                let transactions = block.into_transactions_ecrecovered().take(num_txs);
                for tx in transactions {
                    let tx = tx_env_with_recovered(&tx);
                    let env =
                        EnvWithHandlerCfg::new_with_cfg_env(cfg.clone(), block_env.clone(), tx);
                    let (res, _) = transact(&mut db, env)?;
                    db.commit(res.state);
                }
            }

            let block_overrides = block_override.map(Box::new);

            let mut transactions = transactions.into_iter().peekable();
            while let Some(tx) = transactions.next() {
                // apply state overrides only once, before the first transaction
                let state_overrides = state_override.take();
                let overrides = EvmOverrides::new(state_overrides, block_overrides.clone());

                let env = prepare_call_env(
                    cfg.clone(),
                    block_env.clone(),
                    tx,
                    gas_limit,
                    &mut db,
                    overrides,
                )?;
                let (res, _) = transact(&mut db, env)?;

                match ensure_success(res.result) {
                    Ok(output) => {
                        results.push(EthCallResponse { value: Some(output), error: None });
                    }
                    Err(err) => {
                        results.push(EthCallResponse { value: None, error: Some(err.to_string()) });
                    }
                }

                if transactions.peek().is_some() {
                    // need to apply the state changes of this call before executing the next call
                    db.commit(res.state);
                }
            }

            Ok(results)
        })
        .await
    }

    /// Estimates the gas usage of the `request` with the state.
    ///
    /// This will execute the [TransactionRequest] and find the best gas limit via binary search
    pub fn estimate_gas_with<S>(
        &self,
        mut cfg: CfgEnvWithHandlerCfg,
        block: BlockEnv,
        request: TransactionRequest,
        state: S,
        state_override: Option<StateOverride>,
    ) -> EthResult<U256>
    where
        S: StateProvider,
    {
        // Disabled because eth_estimateGas is sometimes used with eoa senders
        // See <htps://github.com/paradigmxyz/reth/issues/1959>
        cfg.disable_eip3607 = true;

        // The basefee should be ignored for eth_createAccessList
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        cfg.disable_base_fee = true;

        // keep a copy of gas related request values
        let request_gas = request.gas;
        let request_gas_price = request.gas_price;
        let env_gas_limit = block.gas_limit;

        // get the highest possible gas limit, either the request's set value or the currently
        // configured gas limit
        let mut highest_gas_limit = request.gas.unwrap_or(block.gas_limit);

        // Configure the evm env
        let mut env = build_call_evm_env(cfg, block, request)?;
        let mut db = CacheDB::new(StateProviderDatabase::new(state));

        if let Some(state_override) = state_override {
            // apply state overrides
            apply_state_overrides(state_override, &mut db)?;
        }
        // if the request is a simple transfer we can optimize
        if env.tx.data.is_empty() {
            if let TransactTo::Call(to) = env.tx.transact_to {
                if let Ok(code) = db.db.account_code(to) {
                    let no_code_callee = code.map(|code| code.is_empty()).unwrap_or(true);
                    if no_code_callee {
                        // simple transfer, check if caller has sufficient funds
                        let available_funds =
                            db.basic_ref(env.tx.caller)?.map(|acc| acc.balance).unwrap_or_default();
                        if env.tx.value > available_funds {
                            return Err(
                                RpcInvalidTransactionError::InsufficientFundsForTransfer.into()
                            )
                        }
                        return Ok(U256::from(MIN_TRANSACTION_GAS))
                    }
                }
            }
        }

        // check funds of the sender
        if env.tx.gas_price > U256::ZERO {
            let allowance = caller_gas_allowance(&mut db, &env.tx)?;

            if highest_gas_limit > allowance {
                // cap the highest gas limit by max gas caller can afford with given gas price
                highest_gas_limit = allowance;
            }
        }

        // if the provided gas limit is less than computed cap, use that
        let gas_limit = std::cmp::min(U256::from(env.tx.gas_limit), highest_gas_limit);
        env.tx.gas_limit = gas_limit.saturating_to();

        trace!(target: "rpc::eth::estimate", ?env, "Starting gas estimation");

        // transact with the highest __possible__ gas limit
        let ethres = transact(&mut db, env.clone());

        // Exceptional case: init used too much gas, we need to increase the gas limit and try
        // again
        if let Err(EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh)) = ethres
        {
            // if price or limit was included in the request then we can execute the request
            // again with the block's gas limit to check if revert is gas related or not
            if request_gas.is_some() || request_gas_price.is_some() {
                return Err(map_out_of_gas_err(env_gas_limit, env, &mut db))
            }
        }

        let (res, env) = ethres?;
        match res.result {
            ExecutionResult::Success { .. } => {
                // succeeded
            }
            ExecutionResult::Halt { reason, gas_used } => {
                // here we don't check for invalid opcode because already executed with highest gas
                // limit
                return Err(RpcInvalidTransactionError::halt(reason, gas_used).into())
            }
            ExecutionResult::Revert { output, .. } => {
                // if price or limit was included in the request then we can execute the request
                // again with the block's gas limit to check if revert is gas related or not
                return if request_gas.is_some() || request_gas_price.is_some() {
                    Err(map_out_of_gas_err(env_gas_limit, env, &mut db))
                } else {
                    // the transaction did revert
                    Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into())
                }
            }
        }

        // at this point we know the call succeeded but want to find the _best_ (lowest) gas the
        // transaction succeeds with. we  find this by doing a binary search over the
        // possible range NOTE: this is the gas the transaction used, which is less than the
        // transaction requires to succeed
        let gas_used = res.result.gas_used();
        // the lowest value is capped by the gas it takes for a transfer
        let mut lowest_gas_limit =
            if env.tx.transact_to.is_create() { MIN_CREATE_GAS } else { MIN_TRANSACTION_GAS };
        let mut highest_gas_limit: u64 = highest_gas_limit.try_into().unwrap_or(u64::MAX);
        // pick a point that's close to the estimated gas
        let mut mid_gas_limit = std::cmp::min(
            gas_used * 3,
            ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64,
        );

        trace!(target: "rpc::eth::estimate", ?env, ?highest_gas_limit, ?lowest_gas_limit, ?mid_gas_limit, "Starting binary search for gas");

        // binary search
        while (highest_gas_limit - lowest_gas_limit) > 1 {
            let mut env = env.clone();
            env.tx.gas_limit = mid_gas_limit;
            let ethres = transact(&mut db, env);

            // Exceptional case: init used too much gas, we need to increase the gas limit and try
            // again
            if let Err(EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh)) =
                ethres
            {
                // increase the lowest gas limit
                lowest_gas_limit = mid_gas_limit;

                // new midpoint
                mid_gas_limit = ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64;
                continue
            }

            let (res, _) = ethres?;
            match res.result {
                ExecutionResult::Success { .. } => {
                    // cap the highest gas limit with succeeding gas limit
                    highest_gas_limit = mid_gas_limit;
                }
                ExecutionResult::Revert { .. } => {
                    // increase the lowest gas limit
                    lowest_gas_limit = mid_gas_limit;
                }
                ExecutionResult::Halt { reason, .. } => {
                    match reason {
                        HaltReason::OutOfGas(_) | HaltReason::InvalidFEOpcode => {
                            // either out of gas or invalid opcode can be thrown dynamically if
                            // gasLeft is too low, so we treat this as `out of gas`, we know this
                            // call succeeds with a higher gaslimit. common usage of invalid opcode in openzeppelin <https://github.com/OpenZeppelin/openzeppelin-contracts/blob/94697be8a3f0dfcd95dfb13ffbd39b5973f5c65d/contracts/metatx/ERC2771Forwarder.sol#L360-L367>

                            // increase the lowest gas limit
                            lowest_gas_limit = mid_gas_limit;
                        }
                        err => {
                            // these should be unreachable because we know the transaction succeeds,
                            // but we consider these cases an error
                            return Err(RpcInvalidTransactionError::EvmHalt(err).into())
                        }
                    }
                }
            }
            // new midpoint
            mid_gas_limit = ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64;
        }

        Ok(U256::from(highest_gas_limit))
    }

    /// Creates the AccessList for the `request` at the [BlockId] or latest.
    pub(crate) async fn create_access_list_at(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> EthResult<AccessListWithGasUsed> {
        self.on_blocking_task(|this| async move {
            this.create_access_list_with(request, block_number).await
        })
        .await
    }

    async fn create_access_list_with(
        &self,
        mut request: TransactionRequest,
        at: Option<BlockId>,
    ) -> EthResult<AccessListWithGasUsed> {
        let block_id = at.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let (cfg, block, at) = self.evm_env_at(block_id).await?;
        let state = self.state_at(at)?;

        let mut env = build_call_evm_env(cfg, block, request.clone())?;

        // we want to disable this in eth_createAccessList, since this is common practice used by
        // other node impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
        env.cfg.disable_block_gas_limit = true;

        // The basefee should be ignored for eth_createAccessList
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/8990c92aea01ca07801597b00c0d83d4e2d9b811/internal/ethapi/api.go#L1476-L1476>
        env.cfg.disable_base_fee = true;

        let mut db = CacheDB::new(StateProviderDatabase::new(state));

        if request.gas.is_none() && env.tx.gas_price > U256::ZERO {
            // no gas limit was provided in the request, so we need to cap the request's gas limit
            cap_tx_gas_limit_with_caller_allowance(&mut db, &mut env.tx)?;
        }

        let from = request.from.unwrap_or_default();
        let to = if let Some(to) = request.to {
            to
        } else {
            let nonce = db.basic_ref(from)?.unwrap_or_default().nonce;
            from.create(nonce)
        };

        // can consume the list since we're not using the request anymore
        let initial = request.access_list.take().unwrap_or_default();

        let precompiles = get_precompiles(env.handler_cfg.spec_id);
        let mut inspector = AccessListInspector::new(initial, from, to, precompiles);
        let (result, env) = inspect(&mut db, env, &mut inspector)?;

        match result.result {
            ExecutionResult::Halt { reason, .. } => Err(match reason {
                HaltReason::NonceOverflow => RpcInvalidTransactionError::NonceMaxValue,
                halt => RpcInvalidTransactionError::EvmHalt(halt),
            }),
            ExecutionResult::Revert { output, .. } => {
                Err(RpcInvalidTransactionError::Revert(RevertError::new(output)))
            }
            ExecutionResult::Success { .. } => Ok(()),
        }?;

        let access_list = inspector.into_access_list();

        let cfg_with_spec_id =
            CfgEnvWithHandlerCfg { cfg_env: env.cfg.clone(), handler_cfg: env.handler_cfg };

        // calculate the gas used using the access list
        request.access_list = Some(access_list.clone());
        let gas_used =
            self.estimate_gas_with(cfg_with_spec_id, env.block.clone(), request, &*db.db, None)?;

        Ok(AccessListWithGasUsed { access_list, gas_used })
    }
}

/// Executes the requests again after an out of gas error to check if the error is gas related or
/// not
#[inline]
fn map_out_of_gas_err<S>(
    env_gas_limit: U256,
    mut env: EnvWithHandlerCfg,
    mut db: &mut CacheDB<StateProviderDatabase<S>>,
) -> EthApiError
where
    S: StateProvider,
{
    let req_gas_limit = env.tx.gas_limit;
    env.tx.gas_limit = env_gas_limit.try_into().unwrap_or(u64::MAX);
    let (res, _) = match transact(&mut db, env) {
        Ok(res) => res,
        Err(err) => return err,
    };
    match res.result {
        ExecutionResult::Success { .. } => {
            // transaction succeeded by manually increasing the gas limit to
            // highest, which means the caller lacks funds to pay for the tx
            RpcInvalidTransactionError::BasicOutOfGas(U256::from(req_gas_limit)).into()
        }
        ExecutionResult::Revert { output, .. } => {
            // reverted again after bumping the limit
            RpcInvalidTransactionError::Revert(RevertError::new(output)).into()
        }
        ExecutionResult::Halt { reason, .. } => RpcInvalidTransactionError::EvmHalt(reason).into(),
    }
}
