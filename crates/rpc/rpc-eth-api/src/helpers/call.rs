//! Loads a pending block from database. Helper trait for `eth_` transaction, call and trace RPC
//! methods.

use futures::Future;
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    revm_primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, HaltReason,
        ResultAndState, TransactTo,
    },
    Bytes, TransactionSignedEcRecovered, TxKind, B256, U256,
};
use reth_provider::StateProvider;
use reth_revm::{database::StateProviderDatabase, db::CacheDB, DatabaseRef};
use reth_rpc_eth_types::{
    cache::db::{StateCacheDbRefMutWrapper, StateProviderTraitObjWrapper},
    error::ensure_success,
    revm_utils::{
        apply_state_overrides, build_call_evm_env, caller_gas_allowance,
        cap_tx_gas_limit_with_caller_allowance, get_precompiles, prepare_call_env,
    },
    EthApiError, EthResult, RevertError, RpcInvalidTransactionError, StateCacheDb,
};
use reth_rpc_server_types::constants::gas_oracle::{ESTIMATE_GAS_ERROR_RATIO, MIN_TRANSACTION_GAS};
use reth_rpc_types::{
    state::{EvmOverrides, StateOverride},
    AccessListWithGasUsed, BlockId, Bundle, EthCallResponse, StateContext, TransactionInfo,
    TransactionRequest,
};
use revm::{Database, DatabaseCommit};
use revm_inspectors::access_list::AccessListInspector;
use tracing::trace;

use super::{LoadBlock, LoadPendingBlock, LoadState, LoadTransaction, SpawnBlocking, Trace};

/// Execution related functions for the [`EthApiServer`](crate::EthApiServer) trait in
/// the `eth_` namespace.
pub trait EthCall: Call + LoadPendingBlock {
    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = EthResult<U256>> + Send {
        Call::estimate_gas_at(self, request, at, state_override)
    }

    /// Executes the call request (`eth_call`) and returns the output
    fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> impl Future<Output = EthResult<Bytes>> + Send {
        async move {
            let (res, _env) =
                self.transact_call_at(request, block_number.unwrap_or_default(), overrides).await?;

            ensure_success(res.result)
        }
    }

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides
    fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        mut state_override: Option<StateOverride>,
    ) -> impl Future<Output = EthResult<Vec<EthCallResponse>>> + Send
    where
        Self: LoadBlock,
    {
        async move {
            let Bundle { transactions, block_override } = bundle;
            if transactions.is_empty() {
                return Err(EthApiError::InvalidParams(String::from("transactions are empty.")))
            }

            let StateContext { transaction_index, block_number } =
                state_context.unwrap_or_default();
            let transaction_index = transaction_index.unwrap_or_default();

            let target_block = block_number.unwrap_or_default();
            let is_block_target_pending = target_block.is_pending();

            let ((cfg, block_env, _), block) = futures::try_join!(
                self.evm_env_at(target_block),
                self.block_with_senders(target_block)
            )?;

            let Some(block) = block else { return Err(EthApiError::UnknownBlockNumber) };
            let gas_limit = self.call_gas_limit();

            // we're essentially replaying the transactions in the block here, hence we need the
            // state that points to the beginning of the block, which is the state at
            // the parent block
            let mut at = block.parent_hash;
            let mut replay_block_txs = true;

            let num_txs = transaction_index.index().unwrap_or(block.body.len());
            // but if all transactions are to be replayed, we can use the state at the block itself,
            // however only if we're not targeting the pending block, because for pending we can't
            // rely on the block's state being available
            if !is_block_target_pending && num_txs == block.body.len() {
                at = block.hash();
                replay_block_txs = false;
            }

            let this = self.clone();
            self.spawn_with_state_at_block(at.into(), move |state| {
                let mut results = Vec::with_capacity(transactions.len());
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                if replay_block_txs {
                    // only need to replay the transactions in the block if not all transactions are
                    // to be replayed
                    let transactions = block.into_transactions_ecrecovered().take(num_txs);
                    for tx in transactions {
                        let env = EnvWithHandlerCfg::new_with_cfg_env(
                            cfg.clone(),
                            block_env.clone(),
                            Call::evm_config(&this).tx_env(&tx),
                        );
                        let (res, _) = this.transact(&mut db, env)?;
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
                    let (res, _) = this.transact(&mut db, env)?;

                    match ensure_success(res.result) {
                        Ok(output) => {
                            results.push(EthCallResponse { value: Some(output), error: None });
                        }
                        Err(err) => {
                            results.push(EthCallResponse {
                                value: None,
                                error: Some(err.to_string()),
                            });
                        }
                    }

                    if transactions.peek().is_some() {
                        // need to apply the state changes of this call before executing the next
                        // call
                        db.commit(res.state);
                    }
                }

                Ok(results)
            })
            .await
        }
    }

    /// Creates [`AccessListWithGasUsed`] for the [`TransactionRequest`] at the given
    /// [`BlockId`], or latest block.
    fn create_access_list_at(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> impl Future<Output = EthResult<AccessListWithGasUsed>> + Send
    where
        Self: Trace,
    {
        async move {
            let block_id = block_number.unwrap_or_default();
            let (cfg, block, at) = self.evm_env_at(block_id).await?;

            self.spawn_blocking_io(move |this| {
                this.create_access_list_with(cfg, block, at, request)
            })
            .await
        }
    }

    /// Creates [`AccessListWithGasUsed`] for the [`TransactionRequest`] at the given
    /// [`BlockId`].
    fn create_access_list_with(
        &self,
        cfg: CfgEnvWithHandlerCfg,
        block: BlockEnv,
        at: BlockId,
        mut request: TransactionRequest,
    ) -> EthResult<AccessListWithGasUsed>
    where
        Self: Trace,
    {
        let state = self.state_at_block_id(at)?;

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
        let to = if let Some(TxKind::Call(to)) = request.to {
            to
        } else {
            let nonce = db.basic_ref(from)?.unwrap_or_default().nonce;
            from.create(nonce)
        };

        // can consume the list since we're not using the request anymore
        let initial = request.access_list.take().unwrap_or_default();

        let precompiles = get_precompiles(env.handler_cfg.spec_id);
        let mut inspector = AccessListInspector::new(initial, from, to, precompiles);
        let (result, env) = self.inspect(&mut db, env, &mut inspector)?;

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

/// Executes code on state.
pub trait Call: LoadState + SpawnBlocking {
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    ///
    /// Data access in default trait method implementations.
    fn call_gas_limit(&self) -> u64;

    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm;

    /// Executes the closure with the state that corresponds to the given [`BlockId`].
    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderTraitObjWrapper<'_>) -> EthResult<T>,
    {
        let state = self.state_at_block_id(at)?;
        f(StateProviderTraitObjWrapper(&state))
    }

    /// Executes the [`EnvWithHandlerCfg`] against the given [Database] without committing state
    /// changes.
    fn transact<DB>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        DB: Database,
        <DB as Database>::Error: Into<EthApiError>,
    {
        let mut evm = self.evm_config().evm_with_env(db, env);
        let res = evm.transact()?;
        let (_, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env))
    }

    /// Executes the call request at the given [`BlockId`].
    fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = EthResult<(ResultAndState, EnvWithHandlerCfg)>> + Send
    where
        Self: LoadPendingBlock,
    {
        let this = self.clone();
        self.spawn_with_call_at(request, at, overrides, move |db, env| this.transact(db, env))
    }

    /// Executes the closure with the state that corresponds to the given [`BlockId`] on a new task
    fn spawn_with_state_at_block<F, T>(
        &self,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = EthResult<T>> + Send
    where
        F: FnOnce(StateProviderTraitObjWrapper<'_>) -> EthResult<T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_tracing(move |this| {
            let state = this.state_at_block_id(at)?;
            f(StateProviderTraitObjWrapper(&state))
        })
    }

    /// Prepares the state and env for the given [`TransactionRequest`] at the given [`BlockId`] and
    /// executes the closure on a new task returning the result of the closure.
    ///
    /// This returns the configured [`EnvWithHandlerCfg`] for the given [`TransactionRequest`] at
    /// the given [`BlockId`] and with configured call settings: `prepare_call_env`.
    fn spawn_with_call_at<F, R>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        f: F,
    ) -> impl Future<Output = EthResult<R>> + Send
    where
        Self: LoadPendingBlock,
        F: FnOnce(StateCacheDbRefMutWrapper<'_, '_>, EnvWithHandlerCfg) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        async move {
            let (cfg, block_env, at) = self.evm_env_at(at).await?;
            let this = self.clone();
            self.spawn_tracing(move |_| {
                let state = this.state_at_block_id(at)?;
                let mut db =
                    CacheDB::new(StateProviderDatabase::new(StateProviderTraitObjWrapper(&state)));

                let env = prepare_call_env(
                    cfg,
                    block_env,
                    request,
                    this.call_gas_limit(),
                    &mut db,
                    overrides,
                )?;

                f(StateCacheDbRefMutWrapper(&mut db), env)
            })
            .await
            .map_err(|_| EthApiError::InternalBlockingTaskError)
        }
    }

    /// Retrieves the transaction if it exists and executes it.
    ///
    /// Before the transaction is executed, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [`ResultAndState`] after the transaction was executed
    /// and the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [`BlockingTaskPool`](reth_tasks::pool::BlockingTaskPool).
    fn spawn_replay_transaction<F, R>(
        &self,
        hash: B256,
        f: F,
    ) -> impl Future<Output = EthResult<Option<R>>> + Send
    where
        Self: LoadBlock + LoadPendingBlock + LoadTransaction,
        F: FnOnce(TransactionInfo, ResultAndState, StateCacheDb<'_>) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        async move {
            let (transaction, block) = match self.transaction_and_block(hash).await? {
                None => return Ok(None),
                Some(res) => res,
            };
            let (tx, tx_info) = transaction.split();

            let (cfg, block_env, _) = self.evm_env_at(block.hash().into()).await?;

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash;
            let block_txs = block.into_transactions_ecrecovered();

            let this = self.clone();
            self.spawn_with_state_at_block(parent_block.into(), move |state| {
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                // replay all transactions prior to the targeted transaction
                this.replay_transactions_until(
                    &mut db,
                    cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    tx.hash,
                )?;

                let env = EnvWithHandlerCfg::new_with_cfg_env(
                    cfg,
                    block_env,
                    Call::evm_config(&this).tx_env(&tx),
                );

                let (res, _) = this.transact(&mut db, env)?;
                f(tx_info, res, db)
            })
            .await
            .map(Some)
        }
    }

    /// Replays all the transactions until the target transaction is found.
    ///
    /// All transactions before the target transaction are executed and their changes are written to
    /// the _runtime_ db ([`CacheDB`]).
    ///
    /// Note: This assumes the target transaction is in the given iterator.
    /// Returns the index of the target transaction in the given iterator.
    fn replay_transactions_until<DB>(
        &self,
        db: &mut CacheDB<DB>,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        transactions: impl IntoIterator<Item = TransactionSignedEcRecovered>,
        target_tx_hash: B256,
    ) -> Result<usize, EthApiError>
    where
        DB: DatabaseRef,
        EthApiError: From<<DB as DatabaseRef>::Error>,
    {
        let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default());

        let mut evm = self.evm_config().evm_with_env(db, env);
        let mut index = 0;
        for tx in transactions {
            if tx.hash() == target_tx_hash {
                // reached the target transaction
                break
            }

            let sender = tx.signer();
            self.evm_config().fill_tx_env(evm.tx_mut(), &tx.into_signed(), sender);
            evm.transact_commit()?;
            index += 1;
        }
        Ok(index)
    }

    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = EthResult<U256>> + Send
    where
        Self: LoadPendingBlock,
    {
        async move {
            let (cfg, block_env, at) = self.evm_env_at(at).await?;

            self.spawn_blocking_io(move |this| {
                let state = this.state_at_block_id(at)?;
                this.estimate_gas_with(cfg, block_env, request, state, state_override)
            })
            .await
        }
    }

    /// Estimates the gas usage of the `request` with the state.
    ///
    /// This will execute the [`TransactionRequest`] and find the best gas limit via binary search
    fn estimate_gas_with<S>(
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
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        cfg.disable_eip3607 = true;

        // The basefee should be ignored for eth_createAccessList
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        cfg.disable_base_fee = true;

        // Keep a copy of gas related request values
        let tx_request_gas_limit = request.gas;
        let tx_request_gas_price = request.gas_price;
        let block_env_gas_limit = block.gas_limit;

        // Determine the highest possible gas limit, considering both the request's specified limit
        // and the block's limit.
        let mut highest_gas_limit = tx_request_gas_limit
            .map(|tx_gas_limit| U256::from(tx_gas_limit).max(block_env_gas_limit))
            .unwrap_or(block_env_gas_limit);

        // Configure the evm env
        let mut env = build_call_evm_env(cfg, block, request)?;
        let mut db = CacheDB::new(StateProviderDatabase::new(state));

        // Apply any state overrides if specified.
        if let Some(state_override) = state_override {
            apply_state_overrides(state_override, &mut db)?;
        }

        // Optimize for simple transfer transactions, potentially reducing the gas estimate.
        if env.tx.data.is_empty() {
            if let TransactTo::Call(to) = env.tx.transact_to {
                if let Ok(code) = db.db.account_code(to) {
                    let no_code_callee = code.map(|code| code.is_empty()).unwrap_or(true);
                    if no_code_callee {
                        // If the tx is a simple transfer (call to an account with no code) we can
                        // shortcircuit. But simply returning
                        // `MIN_TRANSACTION_GAS` is dangerous because there might be additional
                        // field combos that bump the price up, so we try executing the function
                        // with the minimum gas limit to make sure.
                        let mut env = env.clone();
                        env.tx.gas_limit = MIN_TRANSACTION_GAS;
                        if let Ok((res, _)) = self.transact(&mut db, env) {
                            if res.result.is_success() {
                                return Ok(U256::from(MIN_TRANSACTION_GAS))
                            }
                        }
                    }
                }
            }
        }

        // Check funds of the sender (only useful to check if transaction gas price is more than 0).
        //
        // The caller allowance is check by doing `(account.balance - tx.value) / tx.gas_price`
        if env.tx.gas_price > U256::ZERO {
            // cap the highest gas limit by max gas caller can afford with given gas price
            highest_gas_limit = highest_gas_limit.min(caller_gas_allowance(&mut db, &env.tx)?);
        }

        // We can now normalize the highest gas limit to a u64
        let mut highest_gas_limit: u64 = highest_gas_limit.try_into().unwrap_or(u64::MAX);

        // If the provided gas limit is less than computed cap, use that
        env.tx.gas_limit = env.tx.gas_limit.min(highest_gas_limit);

        trace!(target: "rpc::eth::estimate", ?env, "Starting gas estimation");

        // Execute the transaction with the highest possible gas limit.
        let (mut res, mut env) = match self.transact(&mut db, env.clone()) {
            // Handle the exceptional case where the transaction initialization uses too much gas.
            // If the gas price or gas limit was specified in the request, retry the transaction
            // with the block's gas limit to determine if the failure was due to
            // insufficient gas.
            Err(EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh))
                if tx_request_gas_limit.is_some() || tx_request_gas_price.is_some() =>
            {
                return Err(self.map_out_of_gas_err(block_env_gas_limit, env, &mut db))
            }
            // Propagate other results (successful or other errors).
            ethres => ethres?,
        };

        let gas_refund = match res.result {
            ExecutionResult::Success { gas_refunded, .. } => gas_refunded,
            ExecutionResult::Halt { reason, gas_used } => {
                // here we don't check for invalid opcode because already executed with highest gas
                // limit
                return Err(RpcInvalidTransactionError::halt(reason, gas_used).into())
            }
            ExecutionResult::Revert { output, .. } => {
                // if price or limit was included in the request then we can execute the request
                // again with the block's gas limit to check if revert is gas related or not
                return if tx_request_gas_limit.is_some() || tx_request_gas_price.is_some() {
                    Err(self.map_out_of_gas_err(block_env_gas_limit, env, &mut db))
                } else {
                    // the transaction did revert
                    Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into())
                }
            }
        };

        // At this point we know the call succeeded but want to find the _best_ (lowest) gas the
        // transaction succeeds with. We find this by doing a binary search over the possible range.
        //
        // NOTE: this is the gas the transaction used, which is less than the
        // transaction requires to succeed.
        let mut gas_used = res.result.gas_used();
        // the lowest value is capped by the gas used by the unconstrained transaction
        let mut lowest_gas_limit = gas_used.saturating_sub(1);

        // As stated in Geth, there is a good chance that the transaction will pass if we set the
        // gas limit to the execution gas used plus the gas refund, so we check this first
        // <https://github.com/ethereum/go-ethereum/blob/a5a4fa7032bb248f5a7c40f4e8df2b131c4186a4/eth/gasestimator/gasestimator.go#L135
        //
        // Calculate the optimistic gas limit by adding gas used and gas refund,
        // then applying a 64/63 multiplier to account for gas forwarding rules.
        let optimistic_gas_limit = (gas_used + gas_refund) * 64 / 63;
        if optimistic_gas_limit < highest_gas_limit {
            // Set the transaction's gas limit to the calculated optimistic gas limit.
            env.tx.gas_limit = optimistic_gas_limit;
            // Re-execute the transaction with the new gas limit and update the result and
            // environment.
            (res, env) = self.transact(&mut db, env)?;
            // Update the gas used based on the new result.
            gas_used = res.result.gas_used();
            // Update the gas limit estimates (highest and lowest) based on the execution result.
            self.update_estimated_gas_range(
                res.result,
                optimistic_gas_limit,
                &mut highest_gas_limit,
                &mut lowest_gas_limit,
            )?;
        };

        // Pick a point that's close to the estimated gas
        let mut mid_gas_limit = std::cmp::min(
            gas_used * 3,
            ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64,
        );

        trace!(target: "rpc::eth::estimate", ?env, ?highest_gas_limit, ?lowest_gas_limit, ?mid_gas_limit, "Starting binary search for gas");

        // Binary search narrows the range to find the minimum gas limit needed for the transaction
        // to succeed.
        while (highest_gas_limit - lowest_gas_limit) > 1 {
            // An estimation error is allowed once the current gas limit range used in the binary
            // search is small enough (less than 1.5% of the highest gas limit)
            // <https://github.com/ethereum/go-ethereum/blob/a5a4fa7032bb248f5a7c40f4e8df2b131c4186a4/eth/gasestimator/gasestimator.go#L152
            if (highest_gas_limit - lowest_gas_limit) as f64 / (highest_gas_limit as f64) <
                ESTIMATE_GAS_ERROR_RATIO
            {
                break
            };

            env.tx.gas_limit = mid_gas_limit;

            // Execute transaction and handle potential gas errors, adjusting limits accordingly.
            match self.transact(&mut db, env.clone()) {
                // Check if the error is due to gas being too high.
                Err(EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh)) => {
                    // Increase the lowest gas limit if gas is too high
                    lowest_gas_limit = mid_gas_limit;
                }
                // Handle other cases, including successful transactions.
                ethres => {
                    // Unpack the result and environment if the transaction was successful.
                    (res, env) = ethres?;
                    // Update the estimated gas range based on the transaction result.
                    self.update_estimated_gas_range(
                        res.result,
                        mid_gas_limit,
                        &mut highest_gas_limit,
                        &mut lowest_gas_limit,
                    )?;
                }
            }

            // New midpoint
            mid_gas_limit = ((highest_gas_limit as u128 + lowest_gas_limit as u128) / 2) as u64;
        }

        Ok(U256::from(highest_gas_limit))
    }

    /// Updates the highest and lowest gas limits for binary search based on the execution result.
    ///
    /// This function refines the gas limit estimates used in a binary search to find the optimal
    /// gas limit for a transaction. It adjusts the highest or lowest gas limits depending on
    /// whether the execution succeeded, reverted, or halted due to specific reasons.
    #[inline]
    fn update_estimated_gas_range(
        &self,
        result: ExecutionResult,
        tx_gas_limit: u64,
        highest_gas_limit: &mut u64,
        lowest_gas_limit: &mut u64,
    ) -> EthResult<()> {
        match result {
            ExecutionResult::Success { .. } => {
                // Cap the highest gas limit with the succeeding gas limit.
                *highest_gas_limit = tx_gas_limit;
            }
            ExecutionResult::Revert { .. } => {
                // Increase the lowest gas limit.
                *lowest_gas_limit = tx_gas_limit;
            }
            ExecutionResult::Halt { reason, .. } => {
                match reason {
                    HaltReason::OutOfGas(_) | HaltReason::InvalidEFOpcode => {
                        // Both `OutOfGas` and `InvalidEFOpcode` can occur dynamically if the gas
                        // left is too low. Treat this as an out of gas
                        // condition, knowing that the call succeeds with a
                        // higher gas limit.
                        //
                        // Common usage of invalid opcode in OpenZeppelin:
                        // <https://github.com/OpenZeppelin/openzeppelin-contracts/blob/94697be8a3f0dfcd95dfb13ffbd39b5973f5c65d/contracts/metatx/ERC2771Forwarder.sol#L360-L367>

                        // Increase the lowest gas limit.
                        *lowest_gas_limit = tx_gas_limit;
                    }
                    err => {
                        // These cases should be unreachable because we know the transaction
                        // succeeds, but if they occur, treat them as an
                        // error.
                        return Err(RpcInvalidTransactionError::EvmHalt(err).into())
                    }
                }
            }
        };

        Ok(())
    }

    /// Executes the requests again after an out of gas error to check if the error is gas related
    /// or not
    #[inline]
    fn map_out_of_gas_err<S>(
        &self,
        env_gas_limit: U256,
        mut env: EnvWithHandlerCfg,
        db: &mut CacheDB<StateProviderDatabase<S>>,
    ) -> EthApiError
    where
        S: StateProvider,
    {
        let req_gas_limit = env.tx.gas_limit;
        env.tx.gas_limit = env_gas_limit.try_into().unwrap_or(u64::MAX);
        let (res, _) = match self.transact(db, env) {
            Ok(res) => res,
            Err(err) => return err,
        };
        match res.result {
            ExecutionResult::Success { .. } => {
                // transaction succeeded by manually increasing the gas limit to
                // highest, which means the caller lacks funds to pay for the tx
                RpcInvalidTransactionError::BasicOutOfGas(req_gas_limit).into()
            }
            ExecutionResult::Revert { output, .. } => {
                // reverted again after bumping the limit
                RpcInvalidTransactionError::Revert(RevertError::new(output)).into()
            }
            ExecutionResult::Halt { reason, .. } => {
                RpcInvalidTransactionError::EvmHalt(reason).into()
            }
        }
    }
}
