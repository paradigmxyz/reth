//! Loads a pending block from database. Helper trait for `eth_` transaction, call and trace RPC
//! methods.

use core::fmt;

use super::{LoadBlock, LoadPendingBlock, LoadState, LoadTransaction, SpawnBlocking, Trace};
use crate::{
    helpers::estimate::EstimateCall, FromEvmError, FullEthApiTypes, RpcBlock, RpcNodeCore,
};
use alloy_consensus::{transaction::TxHashRef, BlockHeader};
use alloy_eips::eip2930::AccessListResult;
use alloy_evm::overrides::{apply_block_overrides, apply_state_overrides, OverrideBlockHashes};
use alloy_network::TransactionBuilder;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rpc_types_eth::{
    simulate::{SimBlock, SimulatePayload, SimulatedBlock},
    state::{EvmOverrides, StateOverride},
    BlockId, Bundle, EthCallResponse, StateContext, TransactionInfo,
};
use futures::Future;
use reth_errors::{ProviderError, RethError};
use reth_evm::{
    block::BlockExecutor, env::BlockEnvironment, execute::BlockBuilder, ConfigureEvm, Evm,
    EvmEnvFor, HaltReasonFor, InspectorFor, TransactionEnv, TxEnvFor,
};
use reth_node_api::BlockBody;
use reth_primitives_traits::Recovered;
use reth_revm::{
    cancelled::CancelOnDrop,
    database::StateProviderDatabase,
    db::{bal::EvmDatabaseError, State},
};
use reth_rpc_convert::{RpcConvert, RpcTxReq};
use reth_rpc_eth_types::{
    cache::db::StateProviderTraitObjWrapper,
    error::{AsEthApiError, FromEthApiError},
    simulate::{self, EthSimulateError},
    EthApiError, StateCacheDb,
};
use reth_storage_api::{BlockIdReader, ProviderTx, StateProviderBox};
use revm::{
    context::Block,
    context_interface::{result::ResultAndState, Transaction},
    Database, DatabaseCommit,
};
use revm_inspectors::{access_list::AccessListInspector, transfer::TransferInspector};
use tracing::{trace, warn};

/// Result type for `eth_simulateV1` RPC method.
pub type SimulatedBlocksResult<N, E> = Result<Vec<SimulatedBlock<RpcBlock<N>>>, E>;

/// Execution related functions for the [`EthApiServer`](crate::EthApiServer) trait in
/// the `eth_` namespace.
pub trait EthCall: EstimateCall + Call + LoadPendingBlock + LoadBlock + FullEthApiTypes {
    /// Estimate gas needed for execution of the `request` at the [`BlockId`].
    fn estimate_gas_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<U256, Self::Error>> + Send {
        EstimateCall::estimate_gas_at(self, request, at, state_override)
    }

    /// `eth_simulateV1` executes an arbitrary number of transactions on top of the requested state.
    /// The transactions are packed into individual blocks. Overrides can be provided.
    ///
    /// See also: <https://github.com/ethereum/go-ethereum/pull/27720>
    fn simulate_v1(
        &self,
        payload: SimulatePayload<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>>,
        block: Option<BlockId>,
    ) -> impl Future<Output = SimulatedBlocksResult<Self::NetworkTypes, Self::Error>> + Send {
        async move {
            if payload.block_state_calls.len() > self.max_simulate_blocks() as usize {
                return Err(EthApiError::InvalidParams("too many blocks.".to_string()).into())
            }

            let block = block.unwrap_or_default();

            let SimulatePayload {
                block_state_calls,
                trace_transfers,
                validation,
                return_full_transactions,
            } = payload;

            if block_state_calls.is_empty() {
                return Err(EthApiError::InvalidParams(String::from("calls are empty.")).into())
            }

            let base_block =
                self.recovered_block(block).await?.ok_or(EthApiError::HeaderNotFound(block))?;
            let mut parent = base_block.sealed_header().clone();

            self.spawn_with_state_at_block(block, move |this, mut db| {
                let mut blocks: Vec<SimulatedBlock<RpcBlock<Self::NetworkTypes>>> =
                    Vec::with_capacity(block_state_calls.len());

                // Track previous block number and timestamp for validation
                let mut prev_block_number = parent.number();
                let mut prev_timestamp = parent.timestamp();

                for block in block_state_calls {
                    // Validate block number ordering if overridden
                    if let Some(number) = block.block_overrides.as_ref().and_then(|o| o.number) {
                        let number: u64 = number.try_into().unwrap_or(u64::MAX);
                        if number <= prev_block_number {
                            return Err(EthApiError::other(EthSimulateError::BlockNumberInvalid {
                                got: number,
                                parent: prev_block_number,
                            })
                            .into());
                        }
                    }
                    // Validate timestamp ordering if overridden
                    if let Some(time) = block
                        .block_overrides
                        .as_ref()
                        .and_then(|o| o.time)
                        .filter(|&t| t <= prev_timestamp)
                    {
                        return Err(EthApiError::other(EthSimulateError::BlockTimestampInvalid {
                            got: time,
                            parent: prev_timestamp,
                        })
                        .into());
                    }

                    let mut evm_env = this
                        .evm_config()
                        .next_evm_env(&parent, &this.next_env_attributes(&parent)?)
                        .map_err(RethError::other)
                        .map_err(Self::Error::from_eth_err)?;

                    // Always disable EIP-3607
                    evm_env.cfg_env.disable_eip3607 = true;

                    if !validation {
                        // If not explicitly required, we disable nonce check <https://github.com/paradigmxyz/reth/issues/16108>
                        evm_env.cfg_env.disable_nonce_check = true;
                        evm_env.cfg_env.disable_base_fee = true;
                        evm_env.cfg_env.tx_gas_limit_cap = Some(u64::MAX);
                        evm_env.block_env.inner_mut().basefee = 0;
                    }

                    let SimBlock { block_overrides, state_overrides, calls } = block;

                    // Set prevrandao to zero for simulated blocks by default,
                    // matching spec behavior where MixDigest is zero-initialized.
                    // If user provides an override, it will be applied by apply_block_overrides.
                    evm_env.block_env.inner_mut().prevrandao = Some(B256::ZERO);

                    if let Some(block_overrides) = block_overrides {
                        // ensure we don't allow uncapped gas limit per block
                        if let Some(gas_limit_override) = block_overrides.gas_limit &&
                            gas_limit_override > evm_env.block_env.gas_limit() &&
                            gas_limit_override > this.call_gas_limit()
                        {
                            return Err(EthApiError::other(EthSimulateError::GasLimitReached).into())
                        }
                        apply_block_overrides(
                            block_overrides,
                            &mut db,
                            evm_env.block_env.inner_mut(),
                        );
                    }
                    if let Some(ref state_overrides) = state_overrides {
                        apply_state_overrides(state_overrides.clone(), &mut db)
                            .map_err(Self::Error::from_eth_err)?;
                    }

                    let block_gas_limit = evm_env.block_env.gas_limit();
                    let chain_id = evm_env.cfg_env.chain_id;

                    let default_gas_limit = {
                        let total_specified_gas =
                            calls.iter().filter_map(|tx| tx.as_ref().gas_limit()).sum::<u64>();
                        let txs_without_gas_limit =
                            calls.iter().filter(|tx| tx.as_ref().gas_limit().is_none()).count();

                        if total_specified_gas > block_gas_limit {
                            return Err(EthApiError::Other(Box::new(
                                EthSimulateError::BlockGasLimitExceeded,
                            ))
                            .into())
                        }

                        if txs_without_gas_limit > 0 {
                            // Per spec: "gasLimit: blockGasLimit - soFarUsedGasInBlock"
                            // Divide remaining gas equally among transactions without gas
                            let gas_per_tx = (block_gas_limit - total_specified_gas) /
                                txs_without_gas_limit as u64;
                            // Cap to RPC gas limit, matching spec behavior
                            let call_gas_limit = this.call_gas_limit();
                            if call_gas_limit > 0 {
                                gas_per_tx.min(call_gas_limit)
                            } else {
                                gas_per_tx
                            }
                        } else {
                            0
                        }
                    };

                    let ctx = this
                        .evm_config()
                        .context_for_next_block(&parent, this.next_env_attributes(&parent)?)
                        .map_err(RethError::other)
                        .map_err(Self::Error::from_eth_err)?;
                    let map_err = |e: EthApiError| -> Self::Error {
                        match e.as_simulate_error() {
                            Some(sim_err) => Self::Error::from_eth_err(EthApiError::other(sim_err)),
                            None => Self::Error::from_eth_err(e),
                        }
                    };

                    let (result, results) = if trace_transfers {
                        // prepare inspector to capture transfer inside the evm so they are recorded
                        // and included in logs
                        let inspector = TransferInspector::new(false).with_logs(true);
                        let evm = this
                            .evm_config()
                            .evm_with_env_and_inspector(&mut db, evm_env, inspector);
                        let mut builder = this.evm_config().create_block_builder(evm, &parent, ctx);

                        if let Some(ref state_overrides) = state_overrides {
                            simulate::apply_precompile_overrides(
                                state_overrides,
                                builder.evm_mut().precompiles_mut(),
                            )
                            .map_err(|e| Self::Error::from_eth_err(EthApiError::other(e)))?;
                        }

                        simulate::execute_transactions(
                            builder,
                            calls,
                            default_gas_limit,
                            chain_id,
                            this.converter(),
                        )
                        .map_err(map_err)?
                    } else {
                        let evm = this.evm_config().evm_with_env(&mut db, evm_env);
                        let mut builder = this.evm_config().create_block_builder(evm, &parent, ctx);

                        if let Some(ref state_overrides) = state_overrides {
                            simulate::apply_precompile_overrides(
                                state_overrides,
                                builder.evm_mut().precompiles_mut(),
                            )
                            .map_err(|e| Self::Error::from_eth_err(EthApiError::other(e)))?;
                        }

                        simulate::execute_transactions(
                            builder,
                            calls,
                            default_gas_limit,
                            chain_id,
                            this.converter(),
                        )
                        .map_err(map_err)?
                    };

                    parent = result.block.clone_sealed_header();

                    // Update tracking for next iteration's validation
                    prev_block_number = parent.number();
                    prev_timestamp = parent.timestamp();

                    let block = simulate::build_simulated_block::<Self::Error, _>(
                        result.block,
                        results,
                        return_full_transactions.into(),
                        this.converter(),
                    )?;

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
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        block_number: Option<BlockId>,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send {
        async move {
            let _permit = self.acquire_owned_blocking_io().await;
            let res =
                self.transact_call_at(request, block_number.unwrap_or_default(), overrides).await?;

            Self::Error::ensure_success(res.result)
        }
    }

    /// Simulate arbitrary number of transactions at an arbitrary blockchain index, with the
    /// optionality of state overrides
    fn call_many(
        &self,
        bundles: Vec<Bundle<RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>>>,
        state_context: Option<StateContext>,
        mut state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<Vec<Vec<EthCallResponse>>, Self::Error>> + Send {
        async move {
            // Check if the vector of bundles is empty
            if bundles.is_empty() {
                return Err(EthApiError::InvalidParams(String::from("bundles are empty.")).into());
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
                self.recovered_block(target_block)
            )?;

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

            self.spawn_with_state_at_block(at, move |this, mut db| {
                let mut all_results = Vec::with_capacity(bundles.len());

                if replay_block_txs {
                    // only need to replay the transactions in the block if not all transactions are
                    // to be replayed
                    RpcNodeCore::evm_config(&this)
                        .executor_for_block(&mut db, block.sealed_block())
                        .map_err(RethError::other)
                        .map_err(Self::Error::from_eth_err)?
                        .apply_pre_execution_changes()
                        .map_err(Self::Error::from_eth_err)?;

                    let block_transactions = block.transactions_recovered().take(num_txs);
                    for tx in block_transactions {
                        let tx_env = RpcNodeCore::evm_config(&this).tx_env(tx);
                        let res = this.transact(&mut db, evm_env.clone(), tx_env)?;
                        db.commit(res.state);
                    }
                }

                // transact all bundles
                for (bundle_index, bundle) in bundles.into_iter().enumerate() {
                    let Bundle { transactions, block_override } = bundle;
                    if transactions.is_empty() {
                        // Skip empty bundles
                        continue;
                    }

                    let mut bundle_results = Vec::with_capacity(transactions.len());
                    let block_overrides = block_override.map(Box::new);

                    // transact all transactions in the bundle
                    for (tx_index, tx) in transactions.into_iter().enumerate() {
                        // Apply overrides, state overrides are only applied for the first tx in the
                        // request
                        let overrides =
                            EvmOverrides::new(state_override.take(), block_overrides.clone());

                        let (current_evm_env, prepared_tx) = this
                            .prepare_call_env(evm_env.clone(), tx, &mut db, overrides)
                            .map_err(|err| {
                                Self::Error::from_eth_err(EthApiError::call_many_error(
                                    bundle_index,
                                    tx_index,
                                    err.into(),
                                ))
                            })?;
                        let res = this.transact(&mut db, current_evm_env, prepared_tx).map_err(
                            |err| {
                                Self::Error::from_eth_err(EthApiError::call_many_error(
                                    bundle_index,
                                    tx_index,
                                    err.into(),
                                ))
                            },
                        )?;

                        match Self::Error::ensure_success(res.result) {
                            Ok(output) => {
                                bundle_results
                                    .push(EthCallResponse { value: Some(output), error: None });
                            }
                            Err(err) => {
                                bundle_results.push(EthCallResponse {
                                    value: None,
                                    error: Some(err.to_string()),
                                });
                            }
                        }

                        // Commit state changes after each transaction to allow subsequent calls to
                        // see the updates
                        db.commit(res.state);
                    }

                    all_results.push(bundle_results);
                }

                Ok(all_results)
            })
            .await
        }
    }

    /// Creates [`AccessListResult`] for the [`RpcTxReq`] at the given
    /// [`BlockId`], or latest block.
    fn create_access_list_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<AccessListResult, Self::Error>> + Send
    where
        Self: Trace,
    {
        async move {
            let block_id = block_number.unwrap_or_default();
            let (evm_env, at) = self.evm_env_at(block_id).await?;

            self.spawn_blocking_io_fut(move |this| async move {
                this.create_access_list_with(evm_env, at, request, state_override).await
            })
            .await
        }
    }

    /// Creates [`AccessListResult`] for the [`RpcTxReq`] at the given
    /// [`BlockId`].
    fn create_access_list_with(
        &self,
        mut evm_env: EvmEnvFor<Self::Evm>,
        at: BlockId,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        state_override: Option<StateOverride>,
    ) -> impl Future<Output = Result<AccessListResult, Self::Error>> + Send
    where
        Self: Trace,
    {
        self.spawn_blocking_io_fut(move |this| async move {
            let state = this.state_at_block_id(at).await?;
            let mut db = State::builder().with_database(StateProviderDatabase::new(state)).build();

            if let Some(state_overrides) = state_override {
                apply_state_overrides(state_overrides, &mut db)
                    .map_err(Self::Error::from_eth_err)?;
            }

            let mut tx_env = this.create_txn_env(&evm_env, request.clone(), &mut db)?;

            // we want to disable this in eth_createAccessList, since this is common practice used
            // by other node impls and providers <https://github.com/foundry-rs/foundry/issues/4388>
            evm_env.cfg_env.disable_block_gas_limit = true;

            // The basefee should be ignored for eth_createAccessList
            // See:
            // <https://github.com/ethereum/go-ethereum/blob/8990c92aea01ca07801597b00c0d83d4e2d9b811/internal/ethapi/api.go#L1476-L1476>
            evm_env.cfg_env.disable_base_fee = true;

            // Disabled because eth_createAccessList is sometimes used with non-eoa senders
            evm_env.cfg_env.disable_eip3607 = true;

            if request.as_ref().gas_limit().is_none() && tx_env.gas_price() > 0 {
                let cap = this.caller_gas_allowance(&mut db, &evm_env, &tx_env)?;
                // no gas limit was provided in the request, so we need to cap the request's gas
                // limit
                tx_env.set_gas_limit(cap.min(evm_env.block_env.gas_limit()));
            }

            // can consume the list since we're not using the request anymore
            let initial = request.as_ref().access_list().cloned().unwrap_or_default();

            let mut inspector = AccessListInspector::new(initial);

            let result = this.inspect(&mut db, evm_env.clone(), tx_env.clone(), &mut inspector)?;
            let access_list = inspector.into_access_list();
            let gas_used = result.result.gas_used();
            tx_env.set_access_list(access_list.clone());
            if let Err(err) = Self::Error::ensure_success(result.result) {
                return Ok(AccessListResult {
                    access_list,
                    gas_used: U256::from(gas_used),
                    error: Some(err.to_string()),
                });
            }

            // transact again to get the exact gas used
            let result = this.transact(&mut db, evm_env, tx_env)?;
            let gas_used = result.result.gas_used();
            let error = Self::Error::ensure_success(result.result).err().map(|e| e.to_string());

            Ok(AccessListResult { access_list, gas_used: U256::from(gas_used), error })
        })
    }
}

/// Executes code on state.
pub trait Call:
    LoadState<
        RpcConvert: RpcConvert<Evm = Self::Evm>,
        Error: FromEvmError<Self::Evm>
                   + From<<Self::RpcConvert as RpcConvert>::Error>
                   + From<ProviderError>,
    > + SpawnBlocking
{
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    ///
    /// Data access in default trait method implementations.
    fn call_gas_limit(&self) -> u64;

    /// Returns the maximum number of blocks accepted for `eth_simulateV1`.
    fn max_simulate_blocks(&self) -> u64;

    /// Returns the maximum memory the EVM can allocate per RPC request.
    fn evm_memory_limit(&self) -> u64;

    /// Returns the max gas limit that the caller can afford given a transaction environment.
    fn caller_gas_allowance(
        &self,
        mut db: impl Database<Error: Into<EthApiError>>,
        _evm_env: &EvmEnvFor<Self::Evm>,
        tx_env: &TxEnvFor<Self::Evm>,
    ) -> Result<u64, Self::Error> {
        alloy_evm::call::caller_gas_allowance(&mut db, tx_env).map_err(Self::Error::from_eth_err)
    }

    /// Executes the closure with the state that corresponds to the given [`BlockId`].
    fn with_state_at_block<F, R>(
        &self,
        at: BlockId,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        R: Send + 'static,
        F: FnOnce(Self, StateProviderBox) -> Result<R, Self::Error> + Send + 'static,
    {
        self.spawn_blocking_io_fut(move |this| async move {
            let state = this.state_at_block_id(at).await?;
            f(this, state)
        })
    }

    /// Executes the `TxEnv` against the given [Database] without committing state
    /// changes.
    fn transact<DB>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>> + fmt::Debug,
    {
        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        let res = evm.transact(tx_env).map_err(Self::Error::from_evm_err)?;

        Ok(res)
    }

    /// Executes the [`reth_evm::EvmEnv`] against the given [Database] without committing state
    /// changes.
    fn transact_with_inspector<DB, I>(
        &self,
        db: DB,
        evm_env: EvmEnvFor<Self::Evm>,
        tx_env: TxEnvFor<Self::Evm>,
        inspector: I,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>> + fmt::Debug,
        I: InspectorFor<Self::Evm, DB>,
    {
        let mut evm = self.evm_config().evm_with_env_and_inspector(db, evm_env, inspector);
        let res = evm.transact(tx_env).map_err(Self::Error::from_evm_err)?;

        Ok(res)
    }

    /// Executes the call request at the given [`BlockId`].
    ///
    /// This spawns a new task that obtains the state for the given [`BlockId`] and then transacts
    /// the call [`Self::transact`]. If the future is dropped before the (blocking) transact
    /// call is invoked, then the task is cancelled early, (for example if the request is terminated
    /// early client-side).
    fn transact_call_at(
        &self,
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> impl Future<Output = Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error>> + Send
    where
        Self: LoadPendingBlock,
    {
        async move {
            let guard = CancelOnDrop::default();
            let cancel = guard.clone();
            let this = self.clone();

            let res = self
                .spawn_with_call_at(request, at, overrides, move |db, evm_env, tx_env| {
                    if cancel.is_cancelled() {
                        // callsite dropped the guard
                        return Err(EthApiError::InternalEthError.into())
                    }
                    this.transact(db, evm_env, tx_env)
                })
                .await;
            drop(guard);
            res
        }
    }

    /// Executes the closure with the state that corresponds to the given [`BlockId`] on a new task
    fn spawn_with_state_at_block<F, R>(
        &self,
        at: impl Into<BlockId>,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self, StateCacheDb) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        let at = at.into();
        self.spawn_blocking_io_fut(move |this| async move {
            let state = this.state_at_block_id(at).await?;
            let db = State::builder()
                .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(state)))
                .build();
            f(this, db)
        })
    }

    /// Prepares the state and env for the given [`RpcTxReq`] at the given [`BlockId`] and
    /// executes the closure on a new task returning the result of the closure.
    ///
    /// This returns the configured [`reth_evm::EvmEnv`] for the given [`RpcTxReq`] at
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
        request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        at: BlockId,
        overrides: EvmOverrides,
        f: F,
    ) -> impl Future<Output = Result<R, Self::Error>> + Send
    where
        Self: LoadPendingBlock,
        F: FnOnce(
                &mut StateCacheDb,
                EvmEnvFor<Self::Evm>,
                TxEnvFor<Self::Evm>,
            ) -> Result<R, Self::Error>
            + Send
            + 'static,
        R: Send + 'static,
    {
        async move {
            let (evm_env, at) = self.evm_env_at(at).await?;
            self.spawn_with_state_at_block(at, move |this, mut db| {
                let (evm_env, tx_env) =
                    this.prepare_call_env(evm_env, request, &mut db, overrides)?;

                f(&mut db, evm_env, tx_env)
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
        F: FnOnce(
                TransactionInfo,
                ResultAndState<HaltReasonFor<Self::Evm>>,
                StateCacheDb,
            ) -> Result<R, Self::Error>
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

            let evm_env = self.evm_env_for_header(block.sealed_block().sealed_header())?;

            // we need to get the state of the parent block because we're essentially replaying the
            // block the transaction is included in
            let parent_block = block.parent_hash();

            self.spawn_with_state_at_block(parent_block, move |this, mut db| {
                let block_txs = block.transactions_recovered();

                // apply pre-execution changes (e.g. EIP-4788 beacon root, EIP-2935 blockhashes)
                RpcNodeCore::evm_config(&this)
                    .executor_for_block(&mut db, block.sealed_block())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?
                    .apply_pre_execution_changes()
                    .map_err(Self::Error::from_eth_err)?;

                // replay all transactions prior to the targeted transaction
                this.replay_transactions_until(&mut db, evm_env.clone(), block_txs, *tx.tx_hash())?;

                let tx_env = RpcNodeCore::evm_config(&this).tx_env(tx);

                let res = this.transact(&mut db, evm_env, tx_env)?;
                f(tx_info, res, db)
            })
            .await
            .map(Some)
        }
    }

    /// Replays all the transactions until the target transaction is found.
    ///
    /// All transactions before the target transaction are executed and their changes are written to
    /// the _runtime_ db ([`State`]).
    ///
    /// Note: This assumes the target transaction is in the given iterator.
    /// Returns the index of the target transaction in the given iterator.
    fn replay_transactions_until<'a, DB, I>(
        &self,
        db: &mut DB,
        evm_env: EvmEnvFor<Self::Evm>,
        transactions: I,
        target_tx_hash: B256,
    ) -> Result<usize, Self::Error>
    where
        DB: Database<Error = EvmDatabaseError<ProviderError>> + DatabaseCommit + core::fmt::Debug,
        I: IntoIterator<Item = Recovered<&'a ProviderTx<Self::Provider>>>,
    {
        let mut evm = self.evm_config().evm_with_env(db, evm_env);
        let mut index = 0;
        for tx in transactions {
            if *tx.tx_hash() == target_tx_hash {
                // reached the target transaction
                break
            }

            let tx_env = self.evm_config().tx_env(tx);
            evm.transact_commit(tx_env).map_err(Self::Error::from_evm_err)?;
            index += 1;
        }
        Ok(index)
    }

    ///
    /// All `TxEnv` fields are derived from the given [`RpcTxReq`], if fields are
    /// `None`, they fall back to the [`reth_evm::EvmEnv`]'s settings.
    fn create_txn_env(
        &self,
        evm_env: &EvmEnvFor<Self::Evm>,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        mut db: impl Database<Error: Into<EthApiError>>,
    ) -> Result<TxEnvFor<Self::Evm>, Self::Error> {
        if request.as_ref().nonce().is_none() {
            let nonce = db
                .basic(request.as_ref().from().unwrap_or_default())
                .map_err(Into::into)?
                .map(|acc| acc.nonce)
                .unwrap_or_default();
            request.as_mut().set_nonce(nonce);
        }

        Ok(self.converter().tx_env(request, evm_env)?)
    }

    /// Prepares the [`reth_evm::EvmEnv`] for execution of calls.
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
    #[expect(clippy::type_complexity)]
    fn prepare_call_env<DB>(
        &self,
        mut evm_env: EvmEnvFor<Self::Evm>,
        mut request: RpcTxReq<<Self::RpcConvert as RpcConvert>::Network>,
        db: &mut DB,
        overrides: EvmOverrides,
    ) -> Result<(EvmEnvFor<Self::Evm>, TxEnvFor<Self::Evm>), Self::Error>
    where
        DB: Database + DatabaseCommit + OverrideBlockHashes,
        EthApiError: From<<DB as Database>::Error>,
    {
        // track whether the request has a gas limit set
        let request_has_gas_limit = request.as_ref().gas_limit().is_some();

        if let Some(requested_gas) = request.as_ref().gas_limit() {
            let global_gas_cap = self.call_gas_limit();
            if global_gas_cap != 0 && global_gas_cap < requested_gas {
                warn!(target: "rpc::eth::call", ?request, ?global_gas_cap, "Capping gas limit to global gas cap");
                request.as_mut().set_gas_limit(global_gas_cap);
            }
        } else {
            // cap request's gas limit to call gas limit
            request.as_mut().set_gas_limit(self.call_gas_limit());
        }

        // Disable block gas limit check to allow executing transactions with higher gas limit (call
        // gas limit): https://github.com/paradigmxyz/reth/issues/18577
        evm_env.cfg_env.disable_block_gas_limit = true;

        // Disabled because eth_call is sometimes used with eoa senders
        // See <https://github.com/paradigmxyz/reth/issues/1959>
        evm_env.cfg_env.disable_eip3607 = true;

        // The basefee should be ignored for eth_call
        // See:
        // <https://github.com/ethereum/go-ethereum/blob/ee8e83fa5f6cb261dad2ed0a7bbcde4930c41e6c/internal/ethapi/api.go#L985>
        evm_env.cfg_env.disable_base_fee = true;

        // Disable EIP-7825 transaction gas limit to support larger transactions
        evm_env.cfg_env.tx_gas_limit_cap = Some(u64::MAX);

        // Disable additional fee charges, e.g. opstack operator fee charge
        // See:
        // <https://github.com/paradigmxyz/reth/issues/18470>
        evm_env.cfg_env.disable_fee_charge = true;

        evm_env.cfg_env.memory_limit = self.evm_memory_limit();

        // set nonce to None so that the correct nonce is chosen by the EVM
        request.as_mut().take_nonce();

        if let Some(block_overrides) = overrides.block {
            apply_block_overrides(*block_overrides, db, evm_env.block_env.inner_mut());
        }
        if let Some(state_overrides) = overrides.state {
            apply_state_overrides(state_overrides, db)
                .map_err(EthApiError::from_state_overrides_err)?;
        }

        let mut tx_env = self.create_txn_env(&evm_env, request, &mut *db)?;

        // lower the basefee to 0 to avoid breaking EVM invariants (basefee < gasprice): <https://github.com/ethereum/go-ethereum/blob/355228b011ef9a85ebc0f21e7196f892038d49f0/internal/ethapi/api.go#L700-L704>
        if tx_env.gas_price() == 0 {
            evm_env.block_env.inner_mut().basefee = 0;
        }

        if !request_has_gas_limit {
            // No gas limit was provided in the request, so we need to cap the transaction gas limit
            if tx_env.gas_price() > 0 {
                // If gas price is specified, cap transaction gas limit with caller allowance
                trace!(target: "rpc::eth::call", ?tx_env, "Applying gas limit cap with caller allowance");
                let cap = self.caller_gas_allowance(db, &evm_env, &tx_env)?;
                // ensure we cap gas_limit to the block's
                tx_env.set_gas_limit(cap.min(evm_env.block_env.gas_limit()));
            }
        }

        Ok((evm_env, tx_env))
    }
}
