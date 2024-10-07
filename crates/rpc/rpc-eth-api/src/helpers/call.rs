//! Loads a pending block from database. Helper trait for `eth_` transaction, call and trace RPC
//! methods.

use crate::{
    AsEthApiError, FromEthApiError, FromEvmError, FullEthApiTypes, IntoEthApiError, RpcBlock,
};
use alloy_eips::{eip1559::calc_next_block_base_fee, eip2930::AccessListResult};
use alloy_primitives::{Bytes, TxKind, B256, U256};
use alloy_rpc_types::{
    simulate::{SimBlock, SimulatePayload, SimulatedBlock},
    state::{EvmOverrides, StateOverride},
    BlockId, Bundle, EthCallResponse, StateContext, TransactionInfo,
};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use futures::Future;
use reth_chainspec::{EthChainSpec, MIN_TRANSACTION_GAS};
use reth_evm::{ConfigureEvm, ConfigureEvmEnv};
use reth_primitives::{
    revm_primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, HaltReason,
        ResultAndState, TransactTo, TxEnv,
    },
    Header, TransactionSignedEcRecovered,
};
use reth_provider::{ChainSpecProvider, HeaderProvider, StateProvider};
use reth_revm::{database::StateProviderDatabase, db::CacheDB, DatabaseRef};
use reth_rpc_eth_types::{
    cache::db::{StateCacheDbRefMutWrapper, StateProviderTraitObjWrapper},
    error::ensure_success,
    revm_utils::{
        apply_block_overrides, apply_state_overrides, caller_gas_allowance,
        cap_tx_gas_limit_with_caller_allowance, get_precompiles, CallFees,
    },
    simulate::{self, EthSimulateError},
    EthApiError, RevertError, RpcInvalidTransactionError, StateCacheDb,
};
use reth_rpc_server_types::constants::gas_oracle::{CALL_STIPEND_GAS, ESTIMATE_GAS_ERROR_RATIO};
use revm::{Database, DatabaseCommit, GetInspector};
use revm_inspectors::{access_list::AccessListInspector, transfer::TransferInspector};
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
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        Call::estimate_gas_at(self, request, at, state_override)
    }

    /// `eth_simulateV1` executes an arbitrary number of transactions on top of the requested state.
    /// The transactions are packed into individual blocks. Overrides can be provided.
    ///
    /// See also: <https://github.com/ethereum/go-ethereum/pull/27720>
    fn simulate_v1(
        &self,
        payload: SimulatePayload,
        block: Option<BlockId>,
    ) -> impl Future<Output = Result<Vec<SimulatedBlock<RpcBlock<Self::NetworkTypes>>>, Self::Error>>
           + Send
    where
        Self: LoadBlock + FullEthApiTypes,
    {
        async move {
            if payload.block_state_calls.len() > self.max_simulate_blocks() as usize {
                return Err(EthApiError::InvalidParams("too many blocks.".to_string()).into())
            }

            let SimulatePayload {
                block_state_calls,
                trace_transfers,
                validation,
                return_full_transactions,
            } = payload;

            if block_state_calls.is_empty() {
                return Err(EthApiError::InvalidParams(String::from("calls are empty.")).into())
            }

            // Build cfg and block env, we'll reuse those.
            let (mut cfg, mut block_env, block) =
                self.evm_env_at(block.unwrap_or_default()).await?;

            // Gas cap for entire operation
            let total_gas_limit = self.call_gas_limit();

            let base_block = self.block(block).await?.ok_or(EthApiError::HeaderNotFound(block))?;
            let mut parent_hash = base_block.header.hash();
            let total_difficulty = LoadPendingBlock::provider(self)
                .header_td_by_number(block_env.number.to())
                .map_err(Self::Error::from_eth_err)?
                .ok_or(EthApiError::HeaderNotFound(block))?;

            // Only enforce base fee if validation is enabled
            cfg.disable_base_fee = !validation;
            // Always disable EIP-3607
            cfg.disable_eip3607 = true;

            let this = self.clone();
            self.spawn_with_state_at_block(block, move |state| {
                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                let mut blocks: Vec<SimulatedBlock<RpcBlock<Self::NetworkTypes>>> =
                    Vec::with_capacity(block_state_calls.len());
                let mut gas_used = 0;
                for block in block_state_calls {
                    // Increase number and timestamp for every new block
                    block_env.number += U256::from(1);
                    block_env.timestamp += U256::from(1);

                    if validation {
                        let chain_spec = LoadPendingBlock::provider(&this).chain_spec();
                        let base_fee_params =
                            chain_spec.base_fee_params_at_timestamp(block_env.timestamp.to());
                        let base_fee = if let Some(latest) = blocks.last() {
                            let header = &latest.inner.header;
                            calc_next_block_base_fee(
                                header.gas_used,
                                header.gas_limit,
                                header.base_fee_per_gas.unwrap_or_default(),
                                base_fee_params,
                            )
                        } else {
                            base_block
                                .header
                                .next_block_base_fee(base_fee_params)
                                .unwrap_or_default()
                        };
                        block_env.basefee = U256::from(base_fee);
                    } else {
                        block_env.basefee = U256::ZERO;
                    }

                    let SimBlock { block_overrides, state_overrides, mut calls } = block;

                    if let Some(block_overrides) = block_overrides {
                        apply_block_overrides(block_overrides, &mut db, &mut block_env);
                    }
                    if let Some(state_overrides) = state_overrides {
                        apply_state_overrides(state_overrides, &mut db)?;
                    }

                    if (total_gas_limit - gas_used) < block_env.gas_limit.to() {
                        return Err(
                            EthApiError::Other(Box::new(EthSimulateError::GasLimitReached)).into()
                        )
                    }

                    // Resolve transactions, populate missing fields and enforce calls correctness.
                    let transactions = simulate::resolve_transactions(
                        &mut calls,
                        validation,
                        block_env.gas_limit.to(),
                        cfg.chain_id,
                        &mut db,
                    )?;

                    let mut calls = calls.into_iter().peekable();
                    let mut results = Vec::with_capacity(calls.len());

                    while let Some(tx) = calls.next() {
                        let env = this.build_call_evm_env(cfg.clone(), block_env.clone(), tx)?;

                        let (res, env) = {
                            if trace_transfers {
                                this.transact_with_inspector(
                                    &mut db,
                                    env,
                                    TransferInspector::new(false).with_logs(true),
                                )?
                            } else {
                                this.transact(&mut db, env)?
                            }
                        };

                        if calls.peek().is_some() {
                            // need to apply the state changes of this call before executing the
                            // next call
                            db.commit(res.state);
                        }

                        results.push((env.tx.caller, res.result));
                    }

                    let block = simulate::build_block::<Self::TransactionCompat>(
                        results,
                        transactions,
                        &block_env,
                        parent_hash,
                        total_difficulty,
                        return_full_transactions,
                        &db,
                    )?;

                    parent_hash = block.inner.header.hash;
                    gas_used += block.inner.header.gas_used;

                    blocks.push(block);
                }

                Ok(blocks)
            })
            .await
        }
    }

    /// Executes the call request (`eth_call`) and returns the output
    fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send {
        async move {
            let (res, _env) =
                self.transact_call_at(request, block_number.unwrap_or_default(), overrides).await?;

            ensure_success(res.result).map_err(Self::Error::from_eth_err)
        }
    }

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides
    fn call_many(
        &self,
        bundle: Bundle,
        state_context: Option<StateContext>,
        mut state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<Vec<EthCallResponse>, Self::Error>> + Send
    where
        Self: LoadBlock,
    {
        async move {
            let Bundle { transactions, block_override } = bundle;
            if transactions.is_empty() {
                return Err(
                    EthApiError::InvalidParams(String::from("transactions are empty.")).into()
                )
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

            let block = block.ok_or(EthApiError::HeaderNotFound(target_block))?;

            // we're essentially replaying the transactions in the block here, hence we need the
            // state that points to the beginning of the block, which is the state at
            // the parent block
            let mut at = block.parent_hash;
            let mut replay_block_txs = true;

            let num_txs = transaction_index.index().unwrap_or(block.body.transactions.len());
            // but if all transactions are to be replayed, we can use the state at the block itself,
            // however only if we're not targeting the pending block, because for pending we can't
            // rely on the block's state being available
            if !is_block_target_pending && num_txs == block.body.transactions.len() {
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

                    let env = this
                        .prepare_call_env(cfg.clone(), block_env.clone(), tx, &mut db, overrides)
                        .map(Into::into)?;
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

    /// Creates [`AccessListResult`] for the [`TransactionRequest`] at the given
    /// [`BlockId`], or latest block.
    fn create_access_list_at(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
    ) -> impl Future<Output = Result<AccessListResult, Self::Error>> + Send
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

    /// Creates [`AccessListResult`] for the [`TransactionRequest`] at the given
    /// [`BlockId`].
    fn create_access_list_with(
        &self,
        cfg: CfgEnvWithHandlerCfg,
        block: BlockEnv,
        at: BlockId,
        mut request: TransactionRequest,
    ) -> Result<AccessListResult, Self::Error>
    where
        Self: Trace,
    {
        let state = self.state_at_block_id(at)?;

        let mut env = self.build_call_evm_env(cfg, block, request.clone())?;

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
            let nonce =
                db.basic_ref(from).map_err(Self::Error::from_eth_err)?.unwrap_or_default().nonce;
            from.create(nonce)
        };

        // can consume the list since we're not using the request anymore
        let initial = request.access_list.take().unwrap_or_default();

        let precompiles = get_precompiles(env.handler_cfg.spec_id);
        let mut inspector = AccessListInspector::new(initial, from, to, precompiles);

        let (result, mut env) = self.inspect(&mut db, env, &mut inspector)?;
        let access_list = inspector.into_access_list();
        env.tx.access_list = access_list.to_vec();
        match result.result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error =
                    Some(RpcInvalidTransactionError::halt(reason, env.tx.gas_limit).to_string());
                return Ok(AccessListResult { access_list, gas_used: U256::from(gas_used), error })
            }
            ExecutionResult::Revert { output, gas_used } => {
                let error = Some(RevertError::new(output).to_string());
                return Ok(AccessListResult { access_list, gas_used: U256::from(gas_used), error })
            }
            ExecutionResult::Success { .. } => {}
        };

        // transact again to get the exact gas used
        let (result, env) = self.transact(&mut db, env)?;
        let res = match result.result {
            ExecutionResult::Halt { reason, gas_used } => {
                let error =
                    Some(RpcInvalidTransactionError::halt(reason, env.tx.gas_limit).to_string());
                AccessListResult { access_list, gas_used: U256::from(gas_used), error }
            }
            ExecutionResult::Revert { output, gas_used } => {
                let error = Some(RevertError::new(output).to_string());
                AccessListResult { access_list, gas_used: U256::from(gas_used), error }
            }
            ExecutionResult::Success { gas_used, .. } => {
                AccessListResult { access_list, gas_used: U256::from(gas_used), error: None }
            }
        };

        Ok(res)
    }
}

/// Executes code on state.
pub trait Call: LoadState + SpawnBlocking {
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    ///
    /// Data access in default trait method implementations.
    fn call_gas_limit(&self) -> u64;

    /// Returns the maximum number of blocks accepted for `eth_simulateV1`.
    fn max_simulate_blocks(&self) -> u64;

    /// Returns a handle for reading evm config.
    ///
    /// Data access in default (L1) trait method implementations.
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header>;

    /// Executes the closure with the state that corresponds to the given [`BlockId`].
    fn with_state_at_block<F, R>(&self, at: BlockId, f: F) -> Result<R, Self::Error>
    where
        F: FnOnce(StateProviderTraitObjWrapper<'_>) -> Result<R, Self::Error>,
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
    ) -> Result<(ResultAndState, EnvWithHandlerCfg), Self::Error>
    where
        DB: Database,
        EthApiError: From<DB::Error>,
    {
        let mut evm = self.evm_config().evm_with_env(db, env);
        let res = evm.transact().map_err(Self::Error::from_evm_err)?;
        let (_, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env))
    }

    /// Executes the [`EnvWithHandlerCfg`] against the given [Database] without committing state
    /// changes.
    fn transact_with_inspector<DB>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: impl GetInspector<DB>,
    ) -> Result<(ResultAndState, EnvWithHandlerCfg), Self::Error>
    where
        DB: Database,
        EthApiError: From<DB::Error>,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(db, env, inspector);
        let res = evm.transact().map_err(Self::Error::from_evm_err)?;
        let (_, env) = evm.into_db_and_env_with_handler_cfg();
        Ok((res, env))
    }

    /// Executes the call request at the given [`BlockId`].
    fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<(ResultAndState, EnvWithHandlerCfg), Self::Error>> + Send
    where
        Self: LoadPendingBlock,
    {
        let this = self.clone();
        self.spawn_with_call_at(request, at, overrides, move |db, env| this.transact(db, env))
    }

    /// Executes the closure with the state that corresponds to the given [`BlockId`] on a new task
    fn spawn_with_state_at_block<F, R>(
        &self,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(StateProviderTraitObjWrapper<'_>) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
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
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        Self: LoadPendingBlock,
        F: FnOnce(StateCacheDbRefMutWrapper<'_, '_>, EnvWithHandlerCfg) -> Result<R, Self::Error>
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

                let env = this.prepare_call_env(cfg, block_env, request, &mut db, overrides)?;

                f(StateCacheDbRefMutWrapper(&mut db), env)
            })
            .await
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
    ) -> impl Future<Output = Result<Option<R>, Self::Error>> + Send
    where
        Self: LoadBlock + LoadPendingBlock + LoadTransaction,
        F: FnOnce(TransactionInfo, ResultAndState, StateCacheDb<'_>) -> Result<R, Self::Error>
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
    ) -> Result<usize, Self::Error>
    where
        DB: DatabaseRef,
        EthApiError: From<DB::Error>,
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
            evm.transact_commit().map_err(Self::Error::from_evm_err)?;
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
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send
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
    /// This will execute the [`TransactionRequest`] and find the best gas limit via binary search.
    ///
    /// ## EVM settings
    ///
    /// This modifies certain EVM settings to mirror geth's `SkipAccountChecks` when transacting requests, see also: <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/core/state_transition.go#L145-L148>:
    ///
    ///  - `disable_eip3607` is set to `true`
    ///  - `disable_base_fee` is set to `true`
    ///  - `nonce` is set to `None`
    fn estimate_gas_with<S>(
        &self,
        mut cfg: CfgEnvWithHandlerCfg,
        block: BlockEnv,
        mut request: TransactionRequest,
        state: S,
        state_override: Option<StateOverride>,
    ) -> Result<U256, Self::Error>
    where
        S: StateProvider,
    {
        // Disabled because eth_estimateGas is sometimes used with eoa senders
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        cfg.disable_eip3607 = true;

        // The basefee should be ignored for eth_estimateGas and similar
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        cfg.disable_base_fee = true;

        // set nonce to None so that the correct nonce is chosen by the EVM
        request.nonce = None;

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
        let mut env = self.build_call_evm_env(cfg, block, request)?;
        let mut db = CacheDB::new(StateProviderDatabase::new(state));

        // Apply any state overrides if specified.
        if let Some(state_override) = state_override {
            apply_state_overrides(state_override, &mut db).map_err(Self::Error::from_eth_err)?;
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
            highest_gas_limit = highest_gas_limit
                .min(caller_gas_allowance(&mut db, &env.tx).map_err(Self::Error::from_eth_err)?);
        }

        // We can now normalize the highest gas limit to a u64
        let mut highest_gas_limit: u64 = highest_gas_limit
            .try_into()
            .unwrap_or_else(|_| self.provider().chain_spec().max_gas_limit());

        // If the provided gas limit is less than computed cap, use that
        env.tx.gas_limit = env.tx.gas_limit.min(highest_gas_limit);

        trace!(target: "rpc::eth::estimate", ?env, "Starting gas estimation");

        // Execute the transaction with the highest possible gas limit.
        let (mut res, mut env) = match self.transact(&mut db, env.clone()) {
            // Handle the exceptional case where the transaction initialization uses too much gas.
            // If the gas price or gas limit was specified in the request, retry the transaction
            // with the block's gas limit to determine if the failure was due to
            // insufficient gas.
            Err(err)
                if err.is_gas_too_high() &&
                    (tx_request_gas_limit.is_some() || tx_request_gas_price.is_some()) =>
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
                return Err(RpcInvalidTransactionError::halt(reason, gas_used).into_eth_err())
            }
            ExecutionResult::Revert { output, .. } => {
                // if price or limit was included in the request then we can execute the request
                // again with the block's gas limit to check if revert is gas related or not
                return if tx_request_gas_limit.is_some() || tx_request_gas_price.is_some() {
                    Err(self.map_out_of_gas_err(block_env_gas_limit, env, &mut db))
                } else {
                    // the transaction did revert
                    Err(RpcInvalidTransactionError::Revert(RevertError::new(output)).into_eth_err())
                }
            }
        };

        // At this point we know the call succeeded but want to find the _best_ (lowest) gas the
        // transaction succeeds with. We find this by doing a binary search over the possible range.

        // we know the tx succeeded with the configured gas limit, so we can use that as the
        // highest, in case we applied a gas cap due to caller allowance above
        highest_gas_limit = env.tx.gas_limit;

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
        let optimistic_gas_limit = (gas_used + gas_refund + CALL_STIPEND_GAS) * 64 / 63;
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
                Err(err) if err.is_gas_too_high() => {
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
    ) -> Result<(), Self::Error> {
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
                    HaltReason::OutOfGas(_) | HaltReason::InvalidFEOpcode => {
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
                        return Err(RpcInvalidTransactionError::EvmHalt(err).into_eth_err())
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
    ) -> Self::Error
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
                RpcInvalidTransactionError::BasicOutOfGas(req_gas_limit).into_eth_err()
            }
            ExecutionResult::Revert { output, .. } => {
                // reverted again after bumping the limit
                RpcInvalidTransactionError::Revert(RevertError::new(output)).into_eth_err()
            }
            ExecutionResult::Halt { reason, .. } => {
                RpcInvalidTransactionError::EvmHalt(reason).into_eth_err()
            }
        }
    }

    /// Configures a new [`TxEnv`]  for the [`TransactionRequest`]
    ///
    /// All [`TxEnv`] fields are derived from the given [`TransactionRequest`], if fields are
    /// `None`, they fall back to the [`BlockEnv`]'s settings.
    fn create_txn_env(
        &self,
        block_env: &BlockEnv,
        request: TransactionRequest,
    ) -> Result<TxEnv, Self::Error> {
        // Ensure that if versioned hashes are set, they're not empty
        if request.blob_versioned_hashes.as_ref().map_or(false, |hashes| hashes.is_empty()) {
            return Err(RpcInvalidTransactionError::BlobTransactionMissingBlobHashes.into_eth_err())
        }

        let TransactionRequest {
            from,
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input,
            nonce,
            access_list,
            chain_id,
            blob_versioned_hashes,
            max_fee_per_blob_gas,
            authorization_list,
            transaction_type: _,
            sidecar: _,
        } = request;

        let CallFees { max_priority_fee_per_gas, gas_price, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                gas_price.map(U256::from),
                max_fee_per_gas.map(U256::from),
                max_priority_fee_per_gas.map(U256::from),
                block_env.basefee,
                blob_versioned_hashes.as_deref(),
                max_fee_per_blob_gas.map(U256::from),
                block_env.get_blob_gasprice().map(U256::from),
            )?;

        let gas_limit = gas.unwrap_or_else(|| {
            // Use maximum allowed gas limit. The reason for this
            // is that both Erigon and Geth use pre-configured gas cap even if
            // it's possible to derive the gas limit from the block:
            // <https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/cmd/rpcdaemon/commands/trace_adhoc.go#L956
            // https://github.com/ledgerwatch/erigon/blob/eae2d9a79cb70dbe30b3a6b79c436872e4605458/eth/ethconfig/config.go#L94>
            block_env.gas_limit.saturating_to()
        });

        #[allow(clippy::needless_update)]
        let env = TxEnv {
            gas_limit,
            nonce,
            caller: from.unwrap_or_default(),
            gas_price,
            gas_priority_fee: max_priority_fee_per_gas,
            transact_to: to.unwrap_or(TxKind::Create),
            value: value.unwrap_or_default(),
            data: input
                .try_into_unique_input()
                .map_err(Self::Error::from_eth_err)?
                .unwrap_or_default(),
            chain_id,
            access_list: access_list.unwrap_or_default().into(),
            // EIP-4844 fields
            blob_hashes: blob_versioned_hashes.unwrap_or_default(),
            max_fee_per_blob_gas,
            // EIP-7702 fields
            authorization_list: authorization_list.map(Into::into),
            ..Default::default()
        };

        Ok(env)
    }

    /// Creates a new [`EnvWithHandlerCfg`] to be used for executing the [`TransactionRequest`] in
    /// `eth_call`.
    ///
    /// Note: this does _not_ access the Database to check the sender.
    fn build_call_evm_env(
        &self,
        cfg: CfgEnvWithHandlerCfg,
        block: BlockEnv,
        request: TransactionRequest,
    ) -> Result<EnvWithHandlerCfg, Self::Error> {
        let tx = self.create_txn_env(&block, request)?;
        Ok(EnvWithHandlerCfg::new_with_cfg_env(cfg, block, tx))
    }

    /// Prepares the [`EnvWithHandlerCfg`] for execution of calls.
    ///
    /// Does not commit any changes to the underlying database.
    ///
    /// ## EVM settings
    ///
    /// This modifies certain EVM settings to mirror geth's `SkipAccountChecks` when transacting requests, see also: <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/core/state_transition.go#L145-L148>:
    ///
    ///  - `disable_eip3607` is set to `true`
    ///  - `disable_base_fee` is set to `true`
    ///  - `nonce` is set to `None`
    fn prepare_call_env<DB>(
        &self,
        mut cfg: CfgEnvWithHandlerCfg,
        mut block: BlockEnv,
        mut request: TransactionRequest,
        db: &mut CacheDB<DB>,
        overrides: EvmOverrides,
    ) -> Result<EnvWithHandlerCfg, Self::Error>
    where
        DB: DatabaseRef,
        EthApiError: From<<DB as DatabaseRef>::Error>,
    {
        if request.gas > Some(self.call_gas_limit()) {
            // configured gas exceeds limit
            return Err(
                EthApiError::InvalidTransaction(RpcInvalidTransactionError::GasTooHigh).into()
            )
        }

        // Disabled because eth_call is sometimes used with eoa senders
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        cfg.disable_eip3607 = true;

        // The basefee should be ignored for eth_call
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        cfg.disable_base_fee = true;

        // set nonce to None so that the correct nonce is chosen by the EVM
        request.nonce = None;

        if let Some(block_overrides) = overrides.block {
            apply_block_overrides(*block_overrides, db, &mut block);
        }
        if let Some(state_overrides) = overrides.state {
            apply_state_overrides(state_overrides, db)?;
        }

        let request_gas = request.gas;
        let mut env = self.build_call_evm_env(cfg, block, request)?;

        if request_gas.is_none() {
            // No gas limit was provided in the request, so we need to cap the transaction gas limit
            if env.tx.gas_price > U256::ZERO {
                // If gas price is specified, cap transaction gas limit with caller allowance
                trace!(target: "rpc::eth::call", ?env, "Applying gas limit cap with caller allowance");
                let cap = caller_gas_allowance(db, &env.tx)?;
                // ensure we cap gas_limit to the block's
                env.tx.gas_limit = cap.min(env.block.gas_limit).saturating_to();
            }
        }

        Ok(env)
    }
}
