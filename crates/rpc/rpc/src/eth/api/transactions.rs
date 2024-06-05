//! Contains RPC handler implementations specific to transactions

use std::sync::Arc;

use alloy_primitives::TxKind as RpcTransactionKind;
use async_trait::async_trait;
use reth_evm::ConfigureEvm;
use reth_network_api::NetworkInfo;
use reth_primitives::{
    revm::env::tx_env_with_recovered, Address, BlockId, BlockNumberOrTag, Bytes,
    FromRecoveredPooledTransaction, SealedBlock, SealedBlockWithSenders, TransactionSigned,
    TransactionSignedEcRecovered, B256, U256,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ReceiptProvider,
    StateProviderBox, StateProviderFactory, TransactionsProvider,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_types::{
    transaction::{
        EIP1559TransactionRequest, EIP2930TransactionRequest, EIP4844TransactionRequest,
        LegacyTransactionRequest,
    },
    Index, Transaction, TransactionInfo, TransactionRequest, TypedTransactionRequest,
};
use reth_rpc_types_compat::transaction::from_recovered_with_block_context;
use reth_transaction_pool::{TransactionOrigin, TransactionPool};
use revm::{
    db::CacheDB,
    primitives::{
        db::DatabaseCommit, BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, EvmState,
        ExecutionResult, ResultAndState,
    },
    Inspector,
};
use revm_inspectors::tracing::{TracingInspector, TracingInspectorConfig};

use crate::{
    eth::{
        api::{
            pending_block::PendingBlockEnv, BuildReceipt, EthState, EthTransactions, LoadState,
            SpawnBlocking, StateCacheDB,
        },
        cache::EthStateCache,
        error::{EthApiError, EthResult, RpcInvalidTransactionError, SignError},
        revm_utils::{prepare_call_env, EvmOverrides, FillableTransaction},
    },
    EthApi, EthApiSpec,
};

#[async_trait]
impl<Provider, Pool, Network, EvmConfig> EthTransactions
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + 'static,
    Provider: BlockReaderIdExt
        + BlockReader
        + TransactionsProvider
        + ReceiptProvider
        + ChainSpecProvider
        + StateProviderFactory
        + EvmEnvProvider
        + 'static,
    Network: NetworkInfo + 'static,
    EvmConfig: ConfigureEvm,
{
    type Pool = Pool;

    #[inline]
    fn provider(&self) -> &impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }

    #[inline]
    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }

    #[inline]
    fn raw_tx_forwarder(&self) -> &Option<Arc<dyn crate::eth::traits::RawTransactionForwarder>> {
        self.inner.raw_tx_forwarder()
    }

    #[inline]
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap
    }

    async fn spawn_with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        Self: LoadState,
        F: FnOnce(StateProviderBox) -> EthResult<T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_tracing(move |this| {
            let state = this.state_at_block_id(at)?;
            f(state)
        })
        .await
    }

    async fn evm_env_at(&self, at: BlockId) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)>
    where
        Self: LoadState,
    {
        if at.is_pending() {
            let PendingBlockEnv { cfg, block_env, origin } = self.pending_block_env_and_cfg()?;
            Ok((cfg, block_env, origin.state_block_id()))
        } else {
            // Use cached values if there is no pending block
            let block_hash = self
                .provider()
                .block_hash_for_id(at)?
                .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
            let (cfg, env) = self.cache().get_evm_env(block_hash).await?;
            Ok((cfg, env, block_hash.into()))
        }
    }

    async fn block_by_id(&self, id: BlockId) -> EthResult<Option<SealedBlock>> {
        self.block(id).await
    }

    async fn block_by_id_with_senders(
        &self,
        id: BlockId,
    ) -> EthResult<Option<SealedBlockWithSenders>> {
        self.block_with_senders(id).await
    }

    async fn transactions_by_block_id(
        &self,
        block: BlockId,
    ) -> EthResult<Option<Vec<TransactionSigned>>> {
        self.block_by_id(block).await.map(|block| block.map(|block| block.body))
    }

    async fn send_transaction(&self, mut request: TransactionRequest) -> EthResult<B256>
    where
        Self: EthState,
    {
        let from = match request.from {
            Some(from) => from,
            None => return Err(SignError::NoAccount.into()),
        };

        // set nonce if not already set before
        if request.nonce.is_none() {
            let nonce = self.transaction_count(from, Some(BlockId::pending())).await?;
            // note: `.to()` can't panic because the nonce is constructed from a `u64`
            request.nonce = Some(nonce.to::<u64>());
        }

        let chain_id = self.chain_id();

        let estimated_gas = self.estimate_gas_at(request.clone(), BlockId::pending(), None).await?;
        let gas_limit = estimated_gas;

        let TransactionRequest {
            to,
            gas_price,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            gas,
            value,
            input: data,
            nonce,
            mut access_list,
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
            ..
        } = request;

        // todo: remove this inlining after https://github.com/alloy-rs/alloy/pull/183#issuecomment-1928161285
        let transaction = match (
            gas_price,
            max_fee_per_gas,
            access_list.take(),
            max_fee_per_blob_gas,
            blob_versioned_hashes,
            sidecar,
        ) {
            // legacy transaction
            // gas price required
            (Some(_), None, None, None, None, None) => {
                Some(TypedTransactionRequest::Legacy(LegacyTransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: U256::from(gas_price.unwrap_or_default()),
                    gas_limit: U256::from(gas.unwrap_or_default()),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: to.unwrap_or(RpcTransactionKind::Create),
                    chain_id: None,
                }))
            }
            // EIP2930
            // if only accesslist is set, and no eip1599 fees
            (_, None, Some(access_list), None, None, None) => {
                Some(TypedTransactionRequest::EIP2930(EIP2930TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: U256::from(gas_price.unwrap_or_default()),
                    gas_limit: U256::from(gas.unwrap_or_default()),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: to.unwrap_or(RpcTransactionKind::Create),
                    chain_id: 0,
                    access_list,
                }))
            }
            // EIP1559
            // if 4844 fields missing
            // gas_price, max_fee_per_gas, access_list, max_fee_per_blob_gas, blob_versioned_hashes,
            // sidecar,
            (None, _, _, None, None, None) => {
                // Empty fields fall back to the canonical transaction schema.
                Some(TypedTransactionRequest::EIP1559(EIP1559TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    max_fee_per_gas: U256::from(max_fee_per_gas.unwrap_or_default()),
                    max_priority_fee_per_gas: U256::from(
                        max_priority_fee_per_gas.unwrap_or_default(),
                    ),
                    gas_limit: U256::from(gas.unwrap_or_default()),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: to.unwrap_or(RpcTransactionKind::Create),
                    chain_id: 0,
                    access_list: access_list.unwrap_or_default(),
                }))
            }
            // EIP4884
            // all blob fields required
            (
                None,
                _,
                _,
                Some(max_fee_per_blob_gas),
                Some(blob_versioned_hashes),
                Some(sidecar),
            ) => {
                // As per the EIP, we follow the same semantics as EIP-1559.
                Some(TypedTransactionRequest::EIP4844(EIP4844TransactionRequest {
                    chain_id: 0,
                    nonce: nonce.unwrap_or_default(),
                    max_priority_fee_per_gas: U256::from(
                        max_priority_fee_per_gas.unwrap_or_default(),
                    ),
                    max_fee_per_gas: U256::from(max_fee_per_gas.unwrap_or_default()),
                    gas_limit: U256::from(gas.unwrap_or_default()),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    #[allow(clippy::manual_unwrap_or_default)] // clippy is suggesting here unwrap_or_default
                    to: match to {
                        Some(RpcTransactionKind::Call(to)) => to,
                        _ => Address::default(),
                    },
                    access_list: access_list.unwrap_or_default(),

                    // eip-4844 specific.
                    max_fee_per_blob_gas: U256::from(max_fee_per_blob_gas),
                    blob_versioned_hashes,
                    sidecar,
                }))
            }

            _ => None,
        };

        let transaction = match transaction {
            Some(TypedTransactionRequest::Legacy(mut req)) => {
                req.chain_id = Some(chain_id.to());
                req.gas_limit = gas_limit.saturating_to();
                req.gas_price = self.legacy_gas_price(gas_price.map(U256::from)).await?;

                TypedTransactionRequest::Legacy(req)
            }
            Some(TypedTransactionRequest::EIP2930(mut req)) => {
                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit.saturating_to();
                req.gas_price = self.legacy_gas_price(gas_price.map(U256::from)).await?;

                TypedTransactionRequest::EIP2930(req)
            }
            Some(TypedTransactionRequest::EIP1559(mut req)) => {
                let (max_fee_per_gas, max_priority_fee_per_gas) = self
                    .eip1559_fees(
                        max_fee_per_gas.map(U256::from),
                        max_priority_fee_per_gas.map(U256::from),
                    )
                    .await?;

                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit.saturating_to();
                req.max_fee_per_gas = max_fee_per_gas.saturating_to();
                req.max_priority_fee_per_gas = max_priority_fee_per_gas.saturating_to();

                TypedTransactionRequest::EIP1559(req)
            }
            Some(TypedTransactionRequest::EIP4844(mut req)) => {
                let (max_fee_per_gas, max_priority_fee_per_gas) = self
                    .eip1559_fees(
                        max_fee_per_gas.map(U256::from),
                        max_priority_fee_per_gas.map(U256::from),
                    )
                    .await?;

                req.max_fee_per_gas = max_fee_per_gas;
                req.max_priority_fee_per_gas = max_priority_fee_per_gas;
                req.max_fee_per_blob_gas =
                    self.eip4844_blob_fee(max_fee_per_blob_gas.map(U256::from)).await?;

                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit;

                TypedTransactionRequest::EIP4844(req)
            }
            None => return Err(EthApiError::ConflictingFeeFieldsInRequest),
        };

        let signed_tx = self.sign_request(&from, transaction)?;

        let recovered =
            signed_tx.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

        let pool_transaction = match recovered.try_into() {
            Ok(converted) => <Pool::Transaction>::from_recovered_pooled_transaction(converted),
            Err(_) => return Err(EthApiError::TransactionConversionError),
        };

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }

    async fn spawn_with_call_at<F, R>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(&mut StateCacheDB, EnvWithHandlerCfg) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;
        let this = self.clone();
        self.inner
            .blocking_task_pool
            .spawn(move || {
                let state = this.state_at_block_id(at)?;
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let env = prepare_call_env(
                    cfg,
                    block_env,
                    request,
                    this.call_gas_limit(),
                    &mut db,
                    overrides,
                )?;
                f(&mut db, env)
            })
            .await
            .map_err(|_| EthApiError::InternalBlockingTaskError)?
    }

    async fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        Self: LoadState,
    {
        let this = self.clone();
        self.spawn_with_call_at(request, at, overrides, move |db, env| this.transact(db, env)).await
    }

    async fn spawn_inspect_call_at<I>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        Self: LoadState,
        I: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
    {
        let this = self.clone();
        self.spawn_with_call_at(request, at, overrides, move |db, env| {
            this.inspect(db, env, inspector)
        })
        .await
    }

    fn trace_at<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>,
    {
        let this = self.clone();
        self.with_state_at_block(at, |state| {
            let mut db = CacheDB::new(StateProviderDatabase::new(state));
            let mut inspector = TracingInspector::new(config);
            let (res, _) = this.inspect(&mut db, env, &mut inspector)?;
            f(inspector, res)
        })
    }

    async fn spawn_trace_at_with_state<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        Self: LoadState,
        F: FnOnce(TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let this = self.clone();
        self.spawn_with_state_at_block(at, move |state| {
            let mut db = CacheDB::new(StateProviderDatabase::new(state));
            let mut inspector = TracingInspector::new(config);
            let (res, _) = this.inspect(&mut db, env, &mut inspector)?;
            f(inspector, res, db)
        })
        .await
    }

    async fn transaction_and_block(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, SealedBlockWithSenders)>> {
        let (transaction, at) = match self.transaction_by_hash_at(hash).await? {
            None => return Ok(None),
            Some(res) => res,
        };

        // Note: this is always either hash or pending
        let block_hash = match at {
            BlockId::Hash(hash) => hash.block_hash,
            _ => return Ok(None),
        };
        let block = self.cache().get_block_with_senders(block_hash).await?;
        Ok(block.map(|block| (transaction, block.seal(block_hash))))
    }

    async fn spawn_replay_transaction<F, R>(&self, hash: B256, f: F) -> EthResult<Option<R>>
    where
        Self: LoadState,
        F: FnOnce(TransactionInfo, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
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

            let env =
                EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, tx_env_with_recovered(&tx));

            let (res, _) = this.transact(&mut db, env)?;
            f(tx_info, res, db)
        })
        .await
        .map(Some)
    }

    async fn spawn_trace_transaction_in_block_with_inspector<Insp, F, R>(
        &self,
        hash: B256,
        mut inspector: Insp,
        f: F,
    ) -> EthResult<Option<R>>
    where
        Self: LoadState,
        F: FnOnce(TransactionInfo, Insp, ResultAndState, StateCacheDB) -> EthResult<R>
            + Send
            + 'static,
        Insp: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
        R: Send + 'static,
    {
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

            let env =
                EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, tx_env_with_recovered(&tx));

            let (res, _) = this.inspect(&mut db, env, &mut inspector)?;
            f(tx_info, inspector, res, db)
        })
        .await
        .map(Some)
    }

    async fn trace_block_until_with_inspector<Setup, Insp, F, R>(
        &self,
        block_id: BlockId,
        highest_index: Option<u64>,
        mut inspector_setup: Setup,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        Self: LoadState,
        F: for<'a> Fn(
                TransactionInfo,
                Insp,
                ExecutionResult,
                &'a EvmState,
                &'a StateCacheDB,
            ) -> EthResult<R>
            + Send
            + 'static,
        Setup: FnMut() -> Insp + Send + 'static,
        Insp: for<'a> Inspector<&'a mut StateCacheDB> + Send + 'static,
        R: Send + 'static,
    {
        let ((cfg, block_env, _), block) =
            futures::try_join!(self.evm_env_at(block_id), self.block_with_senders(block_id))?;

        let Some(block) = block else { return Ok(None) };

        if block.body.is_empty() {
            // nothing to trace
            return Ok(Some(Vec::new()))
        }

        // replay all transactions of the block
        self.spawn_tracing(move |this| {
            // we need to get the state of the parent block because we're replaying this block on
            // top of its parent block's state
            let state_at = block.parent_hash;
            let block_hash = block.hash();

            let block_number = block_env.number.saturating_to::<u64>();
            let base_fee = block_env.basefee.saturating_to::<u128>();

            // prepare transactions, we do everything upfront to reduce time spent with open state
            let max_transactions = highest_index.map_or(block.body.len(), |highest| {
                // we need + 1 because the index is 0-based
                highest as usize + 1
            });
            let mut results = Vec::with_capacity(max_transactions);

            let mut transactions = block
                .into_transactions_ecrecovered()
                .take(max_transactions)
                .enumerate()
                .map(|(idx, tx)| {
                    let tx_info = TransactionInfo {
                        hash: Some(tx.hash()),
                        index: Some(idx as u64),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee: Some(base_fee),
                    };
                    let tx_env = tx_env_with_recovered(&tx);
                    (tx_info, tx_env)
                })
                .peekable();

            // now get the state
            let state = this.state_at_block_id(state_at.into())?;
            let mut db = CacheDB::new(StateProviderDatabase::new(state));

            while let Some((tx_info, tx)) = transactions.next() {
                let env = EnvWithHandlerCfg::new_with_cfg_env(cfg.clone(), block_env.clone(), tx);

                let mut inspector = inspector_setup();
                let (res, _) = this.inspect(&mut db, env, &mut inspector)?;
                let ResultAndState { result, state } = res;
                results.push(f(tx_info, inspector, result, &state, &db)?);

                // need to apply the state changes of this transaction before executing the
                // next transaction, but only if there's a next transaction
                if transactions.peek().is_some() {
                    // commit the state changes to the DB
                    db.commit(state)
                }
            }

            Ok(results)
        })
        .await
        .map(Some)
    }
}

impl<Provider, Pool, Network, EvmConfig> BuildReceipt
    for EthApi<Provider, Pool, Network, EvmConfig>
{
    #[inline]
    fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }
}

// === impl EthApi ===

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Pool: TransactionPool + 'static,
    Network: NetworkInfo + 'static,
    EvmConfig: ConfigureEvm,
{
    /// Returns the gas price if it is set, otherwise fetches a suggested gas price for legacy
    /// transactions.
    pub(crate) async fn legacy_gas_price(&self, gas_price: Option<U256>) -> EthResult<U256> {
        match gas_price {
            Some(gas_price) => Ok(gas_price),
            None => {
                // fetch a suggested gas price
                self.gas_price().await
            }
        }
    }

    /// Returns the EIP-1559 fees if they are set, otherwise fetches a suggested gas price for
    /// EIP-1559 transactions.
    ///
    /// Returns (`max_fee`, `priority_fee`)
    pub(crate) async fn eip1559_fees(
        &self,
        max_fee_per_gas: Option<U256>,
        max_priority_fee_per_gas: Option<U256>,
    ) -> EthResult<(U256, U256)> {
        let max_fee_per_gas = match max_fee_per_gas {
            Some(max_fee_per_gas) => max_fee_per_gas,
            None => {
                // fetch pending base fee
                let base_fee = self
                    .block(BlockNumberOrTag::Pending)
                    .await?
                    .ok_or(EthApiError::UnknownBlockNumber)?
                    .base_fee_per_gas
                    .ok_or_else(|| {
                        EthApiError::InvalidTransaction(
                            RpcInvalidTransactionError::TxTypeNotSupported,
                        )
                    })?;
                U256::from(base_fee)
            }
        };

        let max_priority_fee_per_gas = match max_priority_fee_per_gas {
            Some(max_priority_fee_per_gas) => max_priority_fee_per_gas,
            None => self.suggested_priority_fee().await?,
        };
        Ok((max_fee_per_gas, max_priority_fee_per_gas))
    }

    /// Returns the EIP-4844 blob fee if it is set, otherwise fetches a blob fee.
    pub(crate) async fn eip4844_blob_fee(&self, blob_fee: Option<U256>) -> EthResult<U256> {
        match blob_fee {
            Some(blob_fee) => Ok(blob_fee),
            None => self.blob_base_fee().await,
        }
    }

    pub(crate) fn sign_request(
        &self,
        from: &Address,
        request: TypedTransactionRequest,
    ) -> EthResult<TransactionSigned> {
        for signer in self.inner.signers.read().iter() {
            if signer.is_signer_for(from) {
                return match signer.sign_transaction(request, from) {
                    Ok(tx) => Ok(tx),
                    Err(e) => Err(e.into()),
                }
            }
        }
        Err(EthApiError::InvalidTransactionSignature)
    }

    /// Get Transaction by [`BlockId`] and the index of the transaction within that Block.
    ///
    /// Returns `Ok(None)` if the block does not exist, or the block as fewer transactions
    pub(crate) async fn transaction_by_block_and_tx_index(
        &self,
        block_id: impl Into<BlockId>,
        index: Index,
    ) -> EthResult<Option<Transaction>> {
        if let Some(block) = self.block_with_senders(block_id.into()).await? {
            let block_hash = block.hash();
            let block_number = block.number;
            let base_fee_per_gas = block.base_fee_per_gas;
            if let Some(tx) = block.into_transactions_ecrecovered().nth(index.into()) {
                return Ok(Some(from_recovered_with_block_context(
                    tx,
                    block_hash,
                    block_number,
                    base_fee_per_gas,
                    index.into(),
                )))
            }
        }

        Ok(None)
    }

    pub(crate) async fn raw_transaction_by_block_and_tx_index(
        &self,
        block_id: impl Into<BlockId>,
        index: Index,
    ) -> EthResult<Option<Bytes>> {
        if let Some(block) = self.block_with_senders(block_id.into()).await? {
            if let Some(tx) = block.transactions().nth(index.into()) {
                return Ok(Some(tx.envelope_encoded()))
            }
        }

        Ok(None)
    }
}

/// Represents from where a transaction was fetched.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TransactionSource {
    /// Transaction exists in the pool (Pending)
    Pool(TransactionSignedEcRecovered),
    /// Transaction already included in a block
    ///
    /// This can be a historical block or a pending block (received from the CL)
    Block {
        /// Transaction fetched via provider
        transaction: TransactionSignedEcRecovered,
        /// Index of the transaction in the block
        index: u64,
        /// Hash of the block.
        block_hash: B256,
        /// Number of the block.
        block_number: u64,
        /// base fee of the block.
        base_fee: Option<u64>,
    },
}

// === impl TransactionSource ===

impl TransactionSource {
    /// Consumes the type and returns the wrapped transaction.
    pub fn into_recovered(self) -> TransactionSignedEcRecovered {
        self.into()
    }

    /// Returns the transaction and block related info, if not pending
    pub fn split(self) -> (TransactionSignedEcRecovered, TransactionInfo) {
        match self {
            Self::Pool(tx) => {
                let hash = tx.hash();
                (
                    tx,
                    TransactionInfo {
                        hash: Some(hash),
                        index: None,
                        block_hash: None,
                        block_number: None,
                        base_fee: None,
                    },
                )
            }
            Self::Block { transaction, index, block_hash, block_number, base_fee } => {
                let hash = transaction.hash();
                (
                    transaction,
                    TransactionInfo {
                        hash: Some(hash),
                        index: Some(index),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee: base_fee.map(u128::from),
                    },
                )
            }
        }
    }
}

impl From<TransactionSource> for TransactionSignedEcRecovered {
    fn from(value: TransactionSource) -> Self {
        match value {
            TransactionSource::Pool(tx) => tx,
            TransactionSource::Block { transaction, .. } => transaction,
        }
    }
}

impl From<TransactionSource> for Transaction {
    fn from(value: TransactionSource) -> Self {
        match value {
            TransactionSource::Pool(tx) => reth_rpc_types_compat::transaction::from_recovered(tx),
            TransactionSource::Block { transaction, index, block_hash, block_number, base_fee } => {
                from_recovered_with_block_context(
                    transaction,
                    block_hash,
                    block_number,
                    base_fee,
                    index as usize,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::{
        cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache, FeeHistoryCacheConfig,
    };
    use reth_evm_ethereum::EthEvmConfig;
    use reth_network_api::noop::NoopNetwork;
    use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, hex_literal::hex};
    use reth_provider::test_utils::NoopProvider;
    use reth_tasks::pool::BlockingTaskPool;
    use reth_transaction_pool::test_utils::testing_pool;

    #[tokio::test]
    async fn send_raw_transaction() {
        let noop_provider = NoopProvider::default();
        let noop_network_provider = NoopNetwork::default();

        let pool = testing_pool();

        let evm_config = EthEvmConfig::default();
        let cache = EthStateCache::spawn(noop_provider, Default::default(), evm_config);
        let fee_history_cache =
            FeeHistoryCache::new(cache.clone(), FeeHistoryCacheConfig::default());
        let eth_api = EthApi::new(
            noop_provider,
            pool.clone(),
            noop_network_provider,
            cache.clone(),
            GasPriceOracle::new(noop_provider, Default::default(), cache.clone()),
            ETHEREUM_BLOCK_GAS_LIMIT,
            BlockingTaskPool::build().expect("failed to build tracing pool"),
            fee_history_cache,
            evm_config,
            None,
        );

        // https://etherscan.io/tx/0xa694b71e6c128a2ed8e2e0f6770bddbe52e3bb8f10e8472f9a79ab81497a8b5d
        let tx_1 = Bytes::from(hex!("02f871018303579880850555633d1b82520894eee27662c2b8eba3cd936a23f039f3189633e4c887ad591c62bdaeb180c080a07ea72c68abfb8fca1bd964f0f99132ed9280261bdca3e549546c0205e800f7d0a05b4ef3039e9c9b9babc179a1878fb825b5aaf5aed2fa8744854150157b08d6f3"));

        let tx_1_result = eth_api.send_raw_transaction(tx_1).await.unwrap();
        assert_eq!(
            pool.len(),
            1,
            "expect 1 transactions in the pool, but pool size is {}",
            pool.len()
        );

        // https://etherscan.io/tx/0x48816c2f32c29d152b0d86ff706f39869e6c1f01dc2fe59a3c1f9ecf39384694
        let tx_2 = Bytes::from(hex!("02f9043c018202b7843b9aca00850c807d37a08304d21d94ef1c6e67703c7bd7107eed8303fbe6ec2554bf6b881bc16d674ec80000b903c43593564c000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000063e2d99f00000000000000000000000000000000000000000000000000000000000000030b000800000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000006000000000000000000000000000000000000000000000000000000000000000c000000000000000000000000000000000000000000000000000000000000001e0000000000000000000000000000000000000000000000000000000000000004000000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000065717fe021ea67801d1088cc80099004b05b64600000000000000000000000000000000000000000000000001bc16d674ec80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002bc02aaa39b223fe8d0a0e5c4f27ead9083c756cc20001f4a0b86991c6218b36c1d19d4a2e9eb0ce3606eb480000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000000180000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000009e95fd5965fd1f1a6f0d4600000000000000000000000000000000000000000000000000000000000000a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002000000000000000000000000a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48000000000000000000000000428dca9537116148616a5a3e44035af17238fe9dc080a0c6ec1e41f5c0b9511c49b171ad4e04c6bb419c74d99fe9891d74126ec6e4e879a032069a753d7a2cfa158df95421724d24c0e9501593c09905abf3699b4a4405ce"));

        let tx_2_result = eth_api.send_raw_transaction(tx_2).await.unwrap();
        assert_eq!(
            pool.len(),
            2,
            "expect 2 transactions in the pool, but pool size is {}",
            pool.len()
        );

        assert!(pool.get(&tx_1_result).is_some(), "tx1 not found in the pool");
        assert!(pool.get(&tx_2_result).is_some(), "tx2 not found in the pool");
    }
}
