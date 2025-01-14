//! Loads a pending block from database. Helper trait for `eth_` transaction, call and trace RPC
//! methods.

use super::{LoadBlock, LoadPendingBlock, LoadState, LoadTransaction, SpawnBlocking, Trace};
use crate::{
    helpers::estimate::EstimateCall, FromEthApiError, FromEvmError, FullEthApiTypes,
    IntoEthApiError, RpcBlock, RpcNodeCore,
};
use alloy_consensus::BlockHeader;
use alloy_eips::{eip1559::calc_next_block_base_fee, eip2930::AccessListResult};
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_eth::{
    simulate::{SimBlock, SimulatePayload, SimulatedBlock},
    state::{EvmOverrides, StateOverride},
    transaction::TransactionRequest,
    BlockId, Bundle, EthCallResponse, StateContext, TransactionInfo,
};
use futures::Future;
use reth_chainspec::EthChainSpec;
use reth_evm::{env::EvmEnv, ConfigureEvm, ConfigureEvmEnv};
use reth_node_api::BlockBody;
use reth_primitives_traits::SignedTransaction;
use reth_provider::{BlockIdReader, ChainSpecProvider, ProviderHeader};
use reth_revm::{
    database::StateProviderDatabase,
    db::CacheDB,
    primitives::{
        BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult, ResultAndState, TxEnv,
    },
    DatabaseRef,
};
use reth_rpc_eth_types::{
    cache::db::{StateCacheDbRefMutWrapper, StateProviderTraitObjWrapper},
    error::ensure_success,
    revm_utils::{
        apply_block_overrides, apply_state_overrides, caller_gas_allowance, get_precompiles,
        CallFees,
    },
    simulate::{self, EthSimulateError},
    EthApiError, RevertError, RpcInvalidTransactionError, StateCacheDb,
};
use revm::{Database, DatabaseCommit, GetInspector};
use revm_inspectors::{access_list::AccessListInspector, transfer::TransferInspector};
use tracing::trace;

/// Result type for `eth_simulateV1` RPC method.
pub type SimulatedBlocksResult<N, E> = Result<Vec<SimulatedBlock<RpcBlock<N>>>, E>;

/// Execution related functions for the [`EthApiServer`](crate::EthApiServer) trait in
/// the `eth_` namespace.
pub trait EthCall: EstimateCall + Call + LoadPendingBlock + LoadBlock + FullEthApiTypes {
    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        EstimateCall::estimate_gas_at(self, request, at, state_override)
    }

    /// `eth_simulateV1` executes an arbitrary number of transactions on top of the requested state.
    /// The transactions are packed into individual blocks. Overrides can be provided.
    ///
    /// See also: <https://github.com/ethereum/go-ethereum/pull/27720>
    #[allow(clippy::type_complexity)]
    fn simulate_v1(
        &self,
        payload: SimulatePayload,
        block: Option<BlockId>,
    ) -> impl Future<Output = SimulatedBlocksResult<Self::NetworkTypes, Self::Error>> + Send {
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
            let (evm_env, block) = self.evm_env_at(block.unwrap_or_default()).await?;
            let EvmEnv { mut cfg_env_with_handler_cfg, mut block_env } = evm_env;

            // Gas cap for entire operation
            let total_gas_limit = self.call_gas_limit();

            let base_block =
                self.block_with_senders(block).await?.ok_or(EthApiError::HeaderNotFound(block))?;
            let mut parent_hash = base_block.hash();

            // Only enforce base fee if validation is enabled
            cfg_env_with_handler_cfg.disable_base_fee = !validation;
            // Always disable EIP-3607
            cfg_env_with_handler_cfg.disable_eip3607 = true;

            let this = self.clone();
            self.spawn_with_state_at_block(block, move |state| {
                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                let mut gas_used = 0;
                let mut blocks: Vec<SimulatedBlock<RpcBlock<Self::NetworkTypes>>> =
                    Vec::with_capacity(block_state_calls.len());
                let mut block_state_calls = block_state_calls.into_iter().peekable();
                while let Some(block) = block_state_calls.next() {
                    // Increase number and timestamp for every new block
                    block_env.number += U256::from(1);
                    block_env.timestamp += U256::from(1);

                    if validation {
                        let chain_spec = RpcNodeCore::provider(&this).chain_spec();
                        let base_fee_params =
                            chain_spec.base_fee_params_at_timestamp(block_env.timestamp.to());
                        let base_fee = if let Some(latest) = blocks.last() {
                            let header = &latest.inner.header;
                            calc_next_block_base_fee(
                                header.gas_used(),
                                header.gas_limit(),
                                header.base_fee_per_gas().unwrap_or_default(),
                                base_fee_params,
                            )
                        } else {
                            base_block.next_block_base_fee(base_fee_params).unwrap_or_default()
                        };
                        block_env.basefee = U256::from(base_fee);
                    } else {
                        block_env.basefee = U256::ZERO;
                    }

                    let SimBlock { block_overrides, state_overrides, calls } = block;

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

                    let default_gas_limit = {
                        let total_specified_gas = calls.iter().filter_map(|tx| tx.gas).sum::<u64>();
                        let txs_without_gas_limit =
                            calls.iter().filter(|tx| tx.gas.is_none()).count();

                        if total_specified_gas > block_env.gas_limit.to() {
                            return Err(EthApiError::Other(Box::new(
                                EthSimulateError::BlockGasLimitExceeded,
                            ))
                            .into())
                        }

                        if txs_without_gas_limit > 0 {
                            (block_env.gas_limit.to::<u64>() - total_specified_gas) /
                                txs_without_gas_limit as u64
                        } else {
                            0
                        }
                    };

                    let mut calls = calls.into_iter().peekable();
                    let mut transactions = Vec::with_capacity(calls.len());
                    let mut senders = Vec::with_capacity(calls.len());
                    let mut results = Vec::with_capacity(calls.len());

                    while let Some(call) = calls.next() {
                        let sender = call.from.unwrap_or_default();

                        // Resolve transaction, populate missing fields and enforce calls
                        // correctness.
                        let tx = simulate::resolve_transaction(
                            call,
                            validation,
                            default_gas_limit,
                            cfg_env_with_handler_cfg.chain_id,
                            &mut db,
                            this.tx_resp_builder(),
                        )?;

                        let tx_env = this.evm_config().tx_env(&tx, sender);
                        let env = EnvWithHandlerCfg::new_with_cfg_env(
                            cfg_env_with_handler_cfg.clone(),
                            block_env.clone(),
                            tx_env,
                        );

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

                        if calls.peek().is_some() || block_state_calls.peek().is_some() {
                            // need to apply the state changes of this call before executing the
                            // next call
                            db.commit(res.state);
                        }

                        transactions.push(tx);
                        senders.push(env.tx.caller);
                        results.push(res.result);
                    }

                    let (block, _) = this.assemble_block_and_receipts(
                        &block_env,
                        parent_hash,
                        // state root calculation is skipped for performance reasons
                        B256::ZERO,
                        transactions,
                        results.clone(),
                    );

                    let block: SimulatedBlock<RpcBlock<Self::NetworkTypes>> =
                        simulate::build_simulated_block(
                            senders,
                            results,
                            return_full_transactions,
                            this.tx_resp_builder(),
                            block,
                        )?;

                    parent_hash = block.inner.header.hash;
                    gas_used += block.inner.header.gas_used();

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
    ) -> impl Future<Output = Result<Vec<EthCallResponse>, Self::Error>> + Send {
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

            let mut target_block = block_number.unwrap_or_default();
            let is_block_target_pending = target_block.is_pending();

            // if it's not pending, we should always use block_hash over block_number to ensure that
            // different provider calls query data related to the same block.
            if !is_block_target_pending {
                target_block = self
                    .provider()
                    .block_hash_for_id(target_block)
                    .map_err(|_| EthApiError::HeaderNotFound(target_block))?
                    .ok_or_else(|| EthApiError::HeaderNotFound(target_block))?
                    .into();
            }

            let ((evm_env, _), block) = futures::try_join!(
                self.evm_env_at(target_block),
                self.block_with_senders(target_block)
            )?;
            let EvmEnv { cfg_env_with_handler_cfg, block_env } = evm_env;

            let block = block.ok_or(EthApiError::HeaderNotFound(target_block))?;

            // we're essentially replaying the transactions in the block here, hence we need the
            // state that points to the beginning of the block, which is the state at
            // the parent block
            let mut at = block.parent_hash();
            let mut replay_block_txs = true;

            let num_txs =
                transaction_index.index().unwrap_or_else(|| block.body().transactions().len());
            // but if all transactions are to be replayed, we can use the state at the block itself,
            // however only if we're not targeting the pending block, because for pending we can't
            // rely on the block's state being available
            if !is_block_target_pending && num_txs == block.body().transactions().len() {
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
                    let transactions = block.transactions_with_sender().take(num_txs);
                    for (signer, tx) in transactions {
                        let env = EnvWithHandlerCfg::new_with_cfg_env(
                            cfg_env_with_handler_cfg.clone(),
                            block_env.clone(),
                            RpcNodeCore::evm_config(&this).tx_env(tx, *signer),
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

                    let env = this.prepare_call_env(
                        cfg_env_with_handler_cfg.clone(),
                        block_env.clone(),
                        tx,
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
            let (evm_env, at) = self.evm_env_at(block_id).await?;
            let EvmEnv { cfg_env_with_handler_cfg, block_env } = evm_env;

            self.spawn_blocking_io(move |this| {
                this.create_access_list_with(cfg_env_with_handler_cfg, block_env, at, request)
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
            let cap = caller_gas_allowance(&mut db, &env.tx)?;
            // no gas limit was provided in the request, so we need to cap the request's gas limit
            env.tx.gas_limit = cap.min(env.block.gas_limit).saturating_to();
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
pub trait Call:
    LoadState<Evm: ConfigureEvm<Header = ProviderHeader<Self::Provider>>> + SpawnBlocking
{
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    ///
    /// Data access in default trait method implementations.
    fn call_gas_limit(&self) -> u64;

    /// Returns the maximum number of blocks accepted for `eth_simulateV1`.
    fn max_simulate_blocks(&self) -> u64;

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
    ///
    /// This is primarily used by `eth_call`.
    ///
    /// # Blocking behaviour
    ///
    /// This assumes executing the call is relatively more expensive on IO than CPU because it
    /// transacts a single transaction on an empty in memory database. Because `eth_call`s are
    /// usually allowed to consume a lot of gas, this also allows a lot of memory operations so
    /// we assume this is not primarily CPU bound and instead spawn the call on a regular tokio task
    /// instead, where blocking IO is less problematic.
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
            let (evm_env, at) = self.evm_env_at(at).await?;
            let EvmEnv { cfg_env_with_handler_cfg, block_env } = evm_env;
            let this = self.clone();
            self.spawn_blocking_io(move |_| {
                let state = this.state_at_block_id(at)?;
                let mut db =
                    CacheDB::new(StateProviderDatabase::new(StateProviderTraitObjWrapper(&state)));

                let env = this.prepare_call_env(
                    cfg_env_with_handler_cfg,
                    block_env,
                    request,
                    &mut db,
                    overrides,
                )?;

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
        Self: LoadBlock + LoadTransaction,
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

            let (evm_env, _) = self.evm_env_at(block.hash().into()).await?;
            let EvmEnv { cfg_env_with_handler_cfg, block_env } = evm_env;

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash();

            let this = self.clone();
            self.spawn_with_state_at_block(parent_block.into(), move |state| {
                let mut db = CacheDB::new(StateProviderDatabase::new(state));
                let block_txs = block.transactions_with_sender();

                // replay all transactions prior to the targeted transaction
                this.replay_transactions_until(
                    &mut db,
                    cfg_env_with_handler_cfg.clone(),
                    block_env.clone(),
                    block_txs,
                    *tx.tx_hash(),
                )?;

                let env = EnvWithHandlerCfg::new_with_cfg_env(
                    cfg_env_with_handler_cfg,
                    block_env,
                    RpcNodeCore::evm_config(&this).tx_env(tx.tx(), tx.signer()),
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
    fn replay_transactions_until<'a, DB, I>(
        &self,
        db: &mut DB,
        cfg: CfgEnvWithHandlerCfg,
        block_env: BlockEnv,
        transactions: I,
        target_tx_hash: B256,
    ) -> Result<usize, Self::Error>
    where
        DB: Database + DatabaseCommit,
        EthApiError: From<DB::Error>,
        I: IntoIterator<Item = (&'a Address, &'a <Self::Evm as ConfigureEvmEnv>::Transaction)>,
        <Self::Evm as ConfigureEvmEnv>::Transaction: SignedTransaction,
    {
        let env = EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, Default::default());

        let mut evm = self.evm_config().evm_with_env(db, env);
        let mut index = 0;
        for (sender, tx) in transactions {
            if *tx.tx_hash() == target_tx_hash {
                // reached the target transaction
                break
            }

            self.evm_config().fill_tx_env(evm.tx_mut(), tx, *sender);
            evm.transact_commit().map_err(Self::Error::from_evm_err)?;
            index += 1;
        }
        Ok(index)
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
        if request.blob_versioned_hashes.as_ref().is_some_and(|hashes| hashes.is_empty()) {
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
    ///
    /// In addition, this changes the block's gas limit to the configured [`Self::call_gas_limit`].
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

        // apply configured gas cap
        block.gas_limit = U256::from(self.call_gas_limit());

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
