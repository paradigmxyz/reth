//! Contains RPC handler implementations specific to transactions
use crate::{
    eth::{
        api::pending_block::PendingBlockEnv,
        error::{EthApiError, EthResult, RpcInvalidTransactionError, SignError},
        revm_utils::{
            inspect, inspect_and_return_db, prepare_call_env, replay_transactions_until, transact,
            EvmOverrides,
        },
        utils::recover_raw_transaction,
    },
    EthApi, EthApiSpec,
};
use async_trait::async_trait;
use reth_network_api::NetworkInfo;
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::{
    eip4844::calc_blob_gasprice,
    revm::env::{fill_block_env_with_coinbase, tx_env_with_recovered},
    Address, BlockId, BlockNumberOrTag, Bytes, FromRecoveredPooledTransaction, Header,
    IntoRecoveredTransaction, Receipt, SealedBlock, SealedBlockWithSenders,
    TransactionKind::{Call, Create},
    TransactionMeta, TransactionSigned, TransactionSignedEcRecovered, B256, U128, U256, U64,
};
use reth_provider::{
    BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, StateProviderBox, StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase,
    tracing::{TracingInspector, TracingInspectorConfig},
};
use reth_rpc_types::{
    transaction::{
        EIP1559TransactionRequest, EIP2930TransactionRequest, EIP4844TransactionRequest,
        LegacyTransactionRequest,
    },
    Index, Log, Transaction, TransactionInfo, TransactionKind as RpcTransactionKind,
    TransactionReceipt, TransactionRequest, TypedTransactionRequest,
};
use reth_rpc_types_compat::transaction::from_recovered_with_block_context;
use reth_transaction_pool::{TransactionOrigin, TransactionPool};
use revm::{
    db::CacheDB,
    primitives::{
        db::DatabaseCommit, BlockEnv, CfgEnvWithHandlerCfg, EnvWithHandlerCfg, ExecutionResult,
        ResultAndState, SpecId, State,
    },
    Inspector,
};

#[cfg(feature = "optimism")]
use crate::eth::api::optimism::OptimismTxMeta;
#[cfg(feature = "optimism")]
use crate::eth::error::OptimismEthApiError;
#[cfg(feature = "optimism")]
use reth_revm::optimism::RethL1BlockInfo;
#[cfg(feature = "optimism")]
use reth_rpc_types::OptimismTransactionReceiptFields;
#[cfg(feature = "optimism")]
use revm::L1BlockInfo;

/// Helper alias type for the state's [CacheDB]
pub(crate) type StateCacheDB = CacheDB<StateProviderDatabase<StateProviderBox>>;

/// Commonly used transaction related functions for the [EthApi] type in the `eth_` namespace.
///
/// Async functions that are spawned onto the
/// [BlockingTaskPool](crate::blocking_pool::BlockingTaskPool) begin with `spawn_`
///
///
/// ## Calls
///
/// There are subtle differences between when transacting [TransactionRequest]:
///
/// The endpoints `eth_call` and `eth_estimateGas` and `eth_createAccessList` should always
/// __disable__ the base fee check in the [EnvWithHandlerCfg]
/// [Cfg](revm_primitives::CfgEnvWithHandlerCfg).
///
/// The behaviour for tracing endpoints is not consistent across clients.
/// Geth also disables the basefee check for tracing: <https://github.com/ethereum/go-ethereum/blob/bc0b87ca196f92e5af49bd33cc190ef0ec32b197/eth/tracers/api.go#L955-L955>
/// Erigon does not: <https://github.com/ledgerwatch/erigon/blob/aefb97b07d1c4fd32a66097a24eddd8f6ccacae0/turbo/transactions/tracing.go#L209-L209>
///
/// See also <https://github.com/paradigmxyz/reth/issues/6240>
///
/// This implementation follows the behaviour of Geth and disables the basefee check for tracing.
#[async_trait::async_trait]
pub trait EthTransactions: Send + Sync {
    /// Returns default gas limit to use for `eth_call` and tracing RPC methods.
    fn call_gas_limit(&self) -> u64;

    /// Returns the state at the given [BlockId]
    fn state_at(&self, at: BlockId) -> EthResult<StateProviderBox>;

    /// Executes the closure with the state that corresponds to the given [BlockId].
    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T>;

    /// Executes the closure with the state that corresponds to the given [BlockId] on a new task
    async fn spawn_with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T> + Send + 'static,
        T: Send + 'static;

    /// Returns the revm evm env for the requested [BlockId]
    ///
    /// If the [BlockId] this will return the [BlockId] of the block the env was configured
    /// for.
    /// If the [BlockId] is pending, this will return the "Pending" tag, otherwise this returns the
    /// hash of the exact block.
    async fn evm_env_at(&self, at: BlockId)
        -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)>;

    /// Returns the revm evm env for the raw block header
    ///
    /// This is used for tracing raw blocks
    async fn evm_env_for_raw_block(
        &self,
        at: &Header,
    ) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv)>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block(&self, block: B256)
        -> EthResult<Option<Vec<TransactionSigned>>>;

    /// Get the entire block for the given id.
    ///
    /// Returns `None` if block does not exist.
    async fn block_by_id(&self, id: BlockId) -> EthResult<Option<SealedBlock>>;

    /// Get the entire block for the given id.
    ///
    /// Returns `None` if block does not exist.
    async fn block_by_id_with_senders(
        &self,
        id: BlockId,
    ) -> EthResult<Option<SealedBlockWithSenders>>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block_id(
        &self,
        block: BlockId,
    ) -> EthResult<Option<Vec<TransactionSigned>>>;

    /// Returns the EIP-2718 encoded transaction by hash.
    ///
    /// If this is a pooled EIP-4844 transaction, the blob sidecar is included.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn raw_transaction_by_hash(&self, hash: B256) -> EthResult<Option<Bytes>>;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn transaction_by_hash(&self, hash: B256) -> EthResult<Option<TransactionSource>>;

    /// Returns the transaction by including its corresponding [BlockId]
    ///
    /// Note: this supports pending transactions
    async fn transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, BlockId)>>;

    /// Returns the _historical_ transaction and the block it was mined in
    async fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, B256)>>;

    /// Returns the transaction receipt for the given hash.
    ///
    /// Returns None if the transaction does not exist or is pending
    /// Note: The tx receipt is not available for pending transactions.
    async fn transaction_receipt(&self, hash: B256) -> EthResult<Option<TransactionReceipt>>;

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<B256>;

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    async fn send_transaction(&self, request: TransactionRequest) -> EthResult<B256>;

    /// Prepares the state and env for the given [TransactionRequest] at the given [BlockId] and
    /// executes the closure on a new task returning the result of the closure.
    ///
    /// This returns the configured [EnvWithHandlerCfg] for the given [TransactionRequest] at the
    /// given [BlockId] and with configured call settings: `prepare_call_env`.
    async fn spawn_with_call_at<F, R>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(StateCacheDB, EnvWithHandlerCfg) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Executes the call request at the given [BlockId].
    async fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>;

    /// Executes the call request at the given [BlockId] on a new task and returns the result of the
    /// inspect call.
    async fn spawn_inspect_call_at<I>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        I: Inspector<StateCacheDB> + Send + 'static;

    /// Executes the transaction on top of the given [BlockId] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [TracingInspector] and the [ResultAndState] after the
    /// configured [EnvWithHandlerCfg] was inspected.
    ///
    /// Caution: this is blocking
    fn trace_at<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>;

    /// Same as [Self::trace_at] but also provides the used database to the callback.
    ///
    /// Executes the transaction on top of the given [BlockId] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [TracingInspector] and the [ResultAndState] after the
    /// configured [EnvWithHandlerCfg] was inspected.
    async fn spawn_trace_at_with_state<F, R>(
        &self,
        env: EnvWithHandlerCfg,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static;

    /// Fetches the transaction and the transaction's block
    async fn transaction_and_block(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, SealedBlockWithSenders)>>;

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first.
    /// The callback `f` is invoked with the [ResultAndState] after the transaction was executed and
    /// the database that points to the beginning of the transaction.
    ///
    /// Note: Implementers should use a threadpool where blocking is allowed, such as
    /// [BlockingTaskPool](crate::blocking_pool::BlockingTaskPool).
    async fn spawn_trace_transaction_in_block<F, R>(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<R>>
    where
        F: FnOnce(TransactionInfo, TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static;

    /// Executes all transactions of a block and returns a list of callback results invoked for each
    /// transaction in the block.
    ///
    /// This
    /// 1. fetches all transactions of the block
    /// 2. configures the EVM evn
    /// 3. loops over all transactions and executes them
    /// 4. calls the callback with the transaction info, the execution result, the changed state
    /// _after_ the transaction [StateProviderDatabase] and the database that points to the state
    /// right _before_ the transaction.
    async fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        // This is the callback that's invoked for each transaction with
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a State,
                &'a CacheDB<StateProviderDatabase<StateProviderBox>>,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static;

    /// Executes all transactions of a block.
    ///
    /// If a `highest_index` is given, this will only execute the first `highest_index`
    /// transactions, in other words, it will stop executing transactions after the
    /// `highest_index`th transaction.
    async fn trace_block_until<F, R>(
        &self,
        block_id: BlockId,
        highest_index: Option<u64>,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a State,
                &'a CacheDB<StateProviderDatabase<StateProviderBox>>,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static;
}

#[async_trait]
impl<Provider, Pool, Network, EvmConfig> EthTransactions
    for EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    fn call_gas_limit(&self) -> u64 {
        self.inner.gas_cap
    }

    fn state_at(&self, at: BlockId) -> EthResult<StateProviderBox> {
        self.state_at_block_id(at)
    }

    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T>,
    {
        let state = self.state_at(at)?;
        f(state)
    }

    async fn spawn_with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox) -> EthResult<T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_tracing_task_with(move |this| {
            let state = this.state_at(at)?;
            f(state)
        })
        .await
    }

    async fn evm_env_at(
        &self,
        at: BlockId,
    ) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv, BlockId)> {
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

    async fn evm_env_for_raw_block(
        &self,
        header: &Header,
    ) -> EthResult<(CfgEnvWithHandlerCfg, BlockEnv)> {
        // get the parent config first
        let (cfg, mut block_env, _) = self.evm_env_at(header.parent_hash.into()).await?;

        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        fill_block_env_with_coinbase(&mut block_env, header, after_merge, header.beneficiary);

        Ok((cfg, block_env))
    }

    async fn transactions_by_block(
        &self,
        block: B256,
    ) -> EthResult<Option<Vec<TransactionSigned>>> {
        Ok(self.cache().get_block_transactions(block).await?)
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

    async fn raw_transaction_by_hash(&self, hash: B256) -> EthResult<Option<Bytes>> {
        // Note: this is mostly used to fetch pooled transactions so we check the pool first
        if let Some(tx) =
            self.pool().get_pooled_transaction_element(hash).map(|tx| tx.envelope_encoded())
        {
            return Ok(Some(tx));
        }

        self.on_blocking_task(|this| async move {
            Ok(this.provider().transaction_by_hash(hash)?.map(|tx| tx.envelope_encoded()))
        })
        .await
    }

    async fn transaction_by_hash(&self, hash: B256) -> EthResult<Option<TransactionSource>> {
        // Try to find the transaction on disk
        let mut resp = self
            .on_blocking_task(|this| async move {
                match this.provider().transaction_by_hash_with_meta(hash)? {
                    None => Ok(None),
                    Some((tx, meta)) => {
                        // Note: we assume this transaction is valid, because it's mined (or part of
                        // pending block) and already. We don't need to
                        // check for pre EIP-2 because this transaction could be pre-EIP-2.
                        let transaction = tx
                            .into_ecrecovered_unchecked()
                            .ok_or(EthApiError::InvalidTransactionSignature)?;

                        let tx = TransactionSource::Block {
                            transaction,
                            index: meta.index,
                            block_hash: meta.block_hash,
                            block_number: meta.block_number,
                            base_fee: meta.base_fee,
                        };
                        Ok(Some(tx))
                    }
                }
            })
            .await?;

        if resp.is_none() {
            // tx not found on disk, check pool
            if let Some(tx) =
                self.pool().get(&hash).map(|tx| tx.transaction.to_recovered_transaction())
            {
                resp = Some(TransactionSource::Pool(tx));
            }
        }

        Ok(resp)
    }

    async fn transaction_by_hash_at(
        &self,
        transaction_hash: B256,
    ) -> EthResult<Option<(TransactionSource, BlockId)>> {
        match self.transaction_by_hash(transaction_hash).await? {
            None => return Ok(None),
            Some(tx) => {
                let res = match tx {
                    tx @ TransactionSource::Pool(_) => {
                        (tx, BlockId::Number(BlockNumberOrTag::Pending))
                    }
                    TransactionSource::Block {
                        transaction,
                        index,
                        block_hash,
                        block_number,
                        base_fee,
                    } => {
                        let at = BlockId::Hash(block_hash.into());
                        let tx = TransactionSource::Block {
                            transaction,
                            index,
                            block_hash,
                            block_number,
                            base_fee,
                        };
                        (tx, at)
                    }
                };
                Ok(Some(res))
            }
        }
    }

    async fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> EthResult<Option<(TransactionSource, B256)>> {
        match self.transaction_by_hash_at(hash).await? {
            None => Ok(None),
            Some((tx, at)) => Ok(at.as_block_hash().map(|hash| (tx, hash))),
        }
    }

    async fn transaction_receipt(&self, hash: B256) -> EthResult<Option<TransactionReceipt>> {
        let result = self
            .on_blocking_task(|this| async move {
                let (tx, meta) = match this.provider().transaction_by_hash_with_meta(hash)? {
                    Some((tx, meta)) => (tx, meta),
                    None => return Ok(None),
                };

                let receipt = match this.provider().receipt_by_hash(hash)? {
                    Some(recpt) => recpt,
                    None => return Ok(None),
                };

                Ok(Some((tx, meta, receipt)))
            })
            .await?;

        let (tx, meta, receipt) = match result {
            Some((tx, meta, receipt)) => (tx, meta, receipt),
            None => return Ok(None),
        };

        self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<B256> {
        // On optimism, transactions are forwarded directly to the sequencer to be included in
        // blocks that it builds.
        #[cfg(feature = "optimism")]
        self.forward_to_sequencer(&tx).await?;

        let recovered = recover_raw_transaction(tx)?;
        let pool_transaction = <Pool::Transaction>::from_recovered_pooled_transaction(recovered);

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }

    async fn send_transaction(&self, mut request: TransactionRequest) -> EthResult<B256> {
        let from = match request.from {
            Some(from) => from,
            None => return Err(SignError::NoAccount.into()),
        };

        // set nonce if not already set before
        if request.nonce.is_none() {
            let nonce =
                self.get_transaction_count(from, Some(BlockId::Number(BlockNumberOrTag::Pending)))?;
            // note: `.to()` can't panic because the nonce is constructed from a `u64`
            request.nonce = Some(U64::from(nonce.to::<u64>()));
        }

        let chain_id = self.chain_id();

        let estimated_gas = self
            .estimate_gas_at(request.clone(), BlockId::Number(BlockNumberOrTag::Pending), None)
            .await?;
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
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: match to {
                        Some(to) => RpcTransactionKind::Call(to),
                        None => RpcTransactionKind::Create,
                    },
                    chain_id: None,
                }))
            }
            // EIP2930
            // if only accesslist is set, and no eip1599 fees
            (_, None, Some(access_list), None, None, None) => {
                Some(TypedTransactionRequest::EIP2930(EIP2930TransactionRequest {
                    nonce: nonce.unwrap_or_default(),
                    gas_price: gas_price.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: match to {
                        Some(to) => RpcTransactionKind::Call(to),
                        None => RpcTransactionKind::Create,
                    },
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
                    max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                    max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: match to {
                        Some(to) => RpcTransactionKind::Call(to),
                        None => RpcTransactionKind::Create,
                    },
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
                    max_priority_fee_per_gas: max_priority_fee_per_gas.unwrap_or_default(),
                    max_fee_per_gas: max_fee_per_gas.unwrap_or_default(),
                    gas_limit: gas.unwrap_or_default(),
                    value: value.unwrap_or_default(),
                    input: data.into_input().unwrap_or_default(),
                    kind: match to {
                        Some(to) => RpcTransactionKind::Call(to),
                        None => RpcTransactionKind::Create,
                    },
                    access_list: access_list.unwrap_or_default(),

                    // eip-4844 specific.
                    max_fee_per_blob_gas,
                    blob_versioned_hashes,
                    sidecar,
                }))
            }

            _ => None,
        };

        let transaction = match transaction {
            Some(TypedTransactionRequest::Legacy(mut req)) => {
                req.chain_id = Some(chain_id.to());
                req.gas_limit = gas_limit;
                req.gas_price = self.legacy_gas_price(gas_price).await?;

                TypedTransactionRequest::Legacy(req)
            }
            Some(TypedTransactionRequest::EIP2930(mut req)) => {
                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit;
                req.gas_price = self.legacy_gas_price(gas_price).await?;

                TypedTransactionRequest::EIP2930(req)
            }
            Some(TypedTransactionRequest::EIP1559(mut req)) => {
                let (max_fee_per_gas, max_priority_fee_per_gas) =
                    self.eip1559_fees(max_fee_per_gas, max_priority_fee_per_gas).await?;

                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit;
                req.max_fee_per_gas = max_fee_per_gas;
                req.max_priority_fee_per_gas = max_priority_fee_per_gas;

                TypedTransactionRequest::EIP1559(req)
            }
            Some(TypedTransactionRequest::EIP4844(mut req)) => {
                let (max_fee_per_gas, max_priority_fee_per_gas) =
                    self.eip1559_fees(max_fee_per_gas, max_priority_fee_per_gas).await?;

                req.max_fee_per_gas = max_fee_per_gas;
                req.max_priority_fee_per_gas = max_priority_fee_per_gas;
                req.max_fee_per_blob_gas = self.eip4844_blob_fee(max_fee_per_blob_gas).await?;

                req.chain_id = chain_id.to();
                req.gas_limit = gas_limit;

                TypedTransactionRequest::EIP4844(req)
            }
            None => return Err(EthApiError::ConflictingFeeFieldsInRequest),
        };

        let signed_tx = self.sign_request(&from, transaction)?;

        let recovered =
            signed_tx.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

        let pool_transaction =
            <Pool::Transaction>::from_recovered_pooled_transaction(recovered.into());

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
        F: FnOnce(StateCacheDB, EnvWithHandlerCfg) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;
        let this = self.clone();
        self.inner
            .blocking_task_pool
            .spawn(move || {
                let state = this.state_at(at)?;
                let mut db = CacheDB::new(StateProviderDatabase::new(state));

                let env = prepare_call_env(
                    cfg,
                    block_env,
                    request,
                    this.call_gas_limit(),
                    &mut db,
                    overrides,
                )?;
                f(db, env)
            })
            .await
            .map_err(|_| EthApiError::InternalBlockingTaskError)?
    }

    async fn transact_call_at(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)> {
        self.spawn_with_call_at(request, at, overrides, move |mut db, env| transact(&mut db, env))
            .await
    }

    async fn spawn_inspect_call_at<I>(
        &self,
        request: TransactionRequest,
        at: BlockId,
        overrides: EvmOverrides,
        inspector: I,
    ) -> EthResult<(ResultAndState, EnvWithHandlerCfg)>
    where
        I: Inspector<StateCacheDB> + Send + 'static,
    {
        self.spawn_with_call_at(request, at, overrides, move |db, env| inspect(db, env, inspector))
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
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>,
    {
        self.with_state_at_block(at, |state| {
            let db = CacheDB::new(StateProviderDatabase::new(state));

            let mut inspector = TracingInspector::new(config);
            let (res, _) = inspect(db, env, &mut inspector)?;

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
        F: FnOnce(TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R> + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_with_state_at_block(at, move |state| {
            let db = CacheDB::new(StateProviderDatabase::new(state));
            let mut inspector = TracingInspector::new(config);
            let (res, _, db) = inspect_and_return_db(db, env, &mut inspector)?;

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

    async fn spawn_trace_transaction_in_block<F, R>(
        &self,
        hash: B256,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<R>>
    where
        F: FnOnce(TransactionInfo, TracingInspector, ResultAndState, StateCacheDB) -> EthResult<R>
            + Send
            + 'static,
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

        self.spawn_with_state_at_block(parent_block.into(), move |state| {
            let mut db = CacheDB::new(StateProviderDatabase::new(state));

            // replay all transactions prior to the targeted transaction
            replay_transactions_until(&mut db, cfg.clone(), block_env.clone(), block_txs, tx.hash)?;

            let env =
                EnvWithHandlerCfg::new_with_cfg_env(cfg, block_env, tx_env_with_recovered(&tx));

            let mut inspector = TracingInspector::new(config);
            let (res, _, db) = inspect_and_return_db(db, env, &mut inspector)?;
            f(tx_info, inspector, res, db)
        })
        .await
        .map(Some)
    }

    async fn trace_block_with<F, R>(
        &self,
        block_id: BlockId,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        // This is the callback that's invoked for each transaction with
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a State,
                &'a CacheDB<StateProviderDatabase<StateProviderBox>>,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        self.trace_block_until(block_id, None, config, f).await
    }

    async fn trace_block_until<F, R>(
        &self,
        block_id: BlockId,
        highest_index: Option<u64>,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<Vec<R>>>
    where
        F: for<'a> Fn(
                TransactionInfo,
                TracingInspector,
                ExecutionResult,
                &'a State,
                &'a CacheDB<StateProviderDatabase<StateProviderBox>>,
            ) -> EthResult<R>
            + Send
            + 'static,
        R: Send + 'static,
    {
        let ((cfg, block_env, _), block) =
            futures::try_join!(self.evm_env_at(block_id), self.block_with_senders(block_id))?;

        let Some(block) = block else { return Ok(None) };

        // replay all transactions of the block
        self.spawn_tracing_task_with(move |this| {
            // we need to get the state of the parent block because we're replaying this block on
            // top of its parent block's state
            let state_at = block.parent_hash;
            let block_hash = block.hash();

            let block_number = block_env.number.saturating_to::<u64>();
            let base_fee = block_env.basefee.saturating_to::<u64>();

            // prepare transactions, we do everything upfront to reduce time spent with open state
            let max_transactions =
                highest_index.map_or(block.body.len(), |highest| highest as usize);
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
            let state = this.state_at(state_at.into())?;
            let mut db = CacheDB::new(StateProviderDatabase::new(state));

            while let Some((tx_info, tx)) = transactions.next() {
                let env = EnvWithHandlerCfg::new_with_cfg_env(cfg.clone(), block_env.clone(), tx);

                let mut inspector = TracingInspector::new(config);
                let (res, _) = inspect(&mut db, env, &mut inspector)?;
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

// === impl EthApi ===

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    /// Spawns the given closure on a new blocking tracing task
    async fn spawn_tracing_task_with<F, T>(&self, f: F) -> EthResult<T>
    where
        F: FnOnce(Self) -> EthResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let this = self.clone();
        self.inner
            .blocking_task_pool
            .spawn(move || f(this))
            .await
            .map_err(|_| EthApiError::InternalBlockingTaskError)?
    }
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + Clone + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
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
    /// Returns (max_fee, priority_fee)
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
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + 'static,
{
    /// Helper function for `eth_getTransactionReceipt`
    ///
    /// Returns the receipt
    #[cfg(not(feature = "optimism"))]
    pub(crate) async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> EthResult<TransactionReceipt> {
        // get all receipts for the block
        let all_receipts = match self.cache().get_receipts(meta.block_hash).await? {
            Some(recpts) => recpts,
            None => return Err(EthApiError::UnknownBlockNumber),
        };
        build_transaction_receipt_with_block_receipts(tx, meta, receipt, &all_receipts)
    }

    /// Helper function for `eth_getTransactionReceipt` (optimism)
    ///
    /// Returns the receipt
    #[cfg(feature = "optimism")]
    pub(crate) async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> EthResult<TransactionReceipt> {
        let (block, receipts) = self
            .cache()
            .get_block_and_receipts(meta.block_hash)
            .await?
            .ok_or(EthApiError::UnknownBlockNumber)?;

        let block = block.unseal();
        let l1_block_info = reth_revm::optimism::extract_l1_info(&block).ok();
        let optimism_tx_meta = self.build_op_tx_meta(&tx, l1_block_info, block.timestamp)?;

        build_transaction_receipt_with_block_receipts(
            tx,
            meta,
            receipt,
            &receipts,
            optimism_tx_meta,
        )
    }

    /// Builds [OptimismTxMeta] object using the provided [TransactionSigned],
    /// [L1BlockInfo] and `block_timestamp`. The [L1BlockInfo] is used to calculate
    /// the l1 fee and l1 data gas for the transaction.
    /// If the [L1BlockInfo] is not provided, the [OptimismTxMeta] will be empty.
    #[cfg(feature = "optimism")]
    pub(crate) fn build_op_tx_meta(
        &self,
        tx: &TransactionSigned,
        l1_block_info: Option<L1BlockInfo>,
        block_timestamp: u64,
    ) -> EthResult<OptimismTxMeta> {
        let Some(l1_block_info) = l1_block_info else { return Ok(OptimismTxMeta::default()) };

        let mut envelope_buf = bytes::BytesMut::new();
        tx.encode_enveloped(&mut envelope_buf);

        let (l1_fee, l1_data_gas) = if !tx.is_deposit() {
            let inner_l1_fee = l1_block_info
                .l1_tx_data_fee(
                    &self.inner.provider.chain_spec(),
                    block_timestamp,
                    &envelope_buf,
                    tx.is_deposit(),
                )
                .map_err(|_| EthApiError::Optimism(OptimismEthApiError::L1BlockFeeError))?;
            let inner_l1_data_gas = l1_block_info
                .l1_data_gas(&self.inner.provider.chain_spec(), block_timestamp, &envelope_buf)
                .map_err(|_| EthApiError::Optimism(OptimismEthApiError::L1BlockGasError))?;
            (Some(inner_l1_fee), Some(inner_l1_data_gas))
        } else {
            (None, None)
        };

        Ok(OptimismTxMeta::new(Some(l1_block_info), l1_fee, l1_data_gas))
    }

    /// Helper function for `eth_sendRawTransaction` for Optimism.
    ///
    /// Forwards the raw transaction bytes to the configured sequencer endpoint.
    /// This is a no-op if the sequencer endpoint is not configured.
    #[cfg(feature = "optimism")]
    pub async fn forward_to_sequencer(&self, tx: &Bytes) -> EthResult<()> {
        if let Some(endpoint) = self.network().sequencer_endpoint() {
            let body = serde_json::to_string(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_sendRawTransaction",
                "params": [format!("0x{}", alloy_primitives::hex::encode(tx))],
                "id": self.network().chain_id()
            }))
            .map_err(|_| {
                tracing::warn!(
                    target = "rpc::eth",
                    "Failed to serialize transaction for forwarding to sequencer"
                );
                EthApiError::Optimism(OptimismEthApiError::InvalidSequencerTransaction)
            })?;

            self.inner
                .http_client
                .post(endpoint)
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|err| EthApiError::Optimism(OptimismEthApiError::HttpError(err)))?;
        }
        Ok(())
    }
}

impl<Provider, Pool, Network, EvmConfig> EthApi<Provider, Pool, Network, EvmConfig>
where
    Pool: TransactionPool + 'static,
    Provider:
        BlockReaderIdExt + ChainSpecProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
    EvmConfig: ConfigureEvmEnv + 'static,
{
    pub(crate) fn sign_request(
        &self,
        from: &Address,
        request: TypedTransactionRequest,
    ) -> EthResult<TransactionSigned> {
        for signer in self.inner.signers.iter() {
            if signer.is_signer_for(from) {
                return match signer.sign_transaction(request, from) {
                    Ok(tx) => Ok(tx),
                    Err(e) => Err(e.into()),
                }
            }
        }
        Err(EthApiError::InvalidTransactionSignature)
    }

    /// Get Transaction by [BlockId] and the index of the transaction within that Block.
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
            TransactionSource::Pool(tx) => {
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
            TransactionSource::Block { transaction, index, block_hash, block_number, base_fee } => {
                let hash = transaction.hash();
                (
                    transaction,
                    TransactionInfo {
                        hash: Some(hash),
                        index: Some(index),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee,
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
                    U256::from(index),
                )
            }
        }
    }
}

/// Helper function to construct a transaction receipt
///
/// Note: This requires _all_ block receipts because we need to calculate the gas used by the
/// transaction.
pub(crate) fn build_transaction_receipt_with_block_receipts(
    transaction: TransactionSigned,
    meta: TransactionMeta,
    receipt: Receipt,
    all_receipts: &[Receipt],
    #[cfg(feature = "optimism")] optimism_tx_meta: OptimismTxMeta,
) -> EthResult<TransactionReceipt> {
    // Note: we assume this transaction is valid, because it's mined (or part of pending block) and
    // we don't need to check for pre EIP-2
    let from =
        transaction.recover_signer_unchecked().ok_or(EthApiError::InvalidTransactionSignature)?;

    // get the previous transaction cumulative gas used
    let gas_used = if meta.index == 0 {
        receipt.cumulative_gas_used
    } else {
        let prev_tx_idx = (meta.index - 1) as usize;
        all_receipts
            .get(prev_tx_idx)
            .map(|prev_receipt| receipt.cumulative_gas_used - prev_receipt.cumulative_gas_used)
            .unwrap_or_default()
    };

    #[allow(clippy::needless_update)]
    let mut res_receipt = TransactionReceipt {
        transaction_hash: Some(meta.tx_hash),
        transaction_index: U64::from(meta.index),
        block_hash: Some(meta.block_hash),
        block_number: Some(U256::from(meta.block_number)),
        from,
        to: None,
        cumulative_gas_used: U256::from(receipt.cumulative_gas_used),
        gas_used: Some(U256::from(gas_used)),
        contract_address: None,
        logs: Vec::with_capacity(receipt.logs.len()),
        effective_gas_price: U128::from(transaction.effective_gas_price(meta.base_fee)),
        transaction_type: transaction.transaction.tx_type().into(),
        // TODO pre-byzantium receipts have a post-transaction state root
        state_root: None,
        logs_bloom: receipt.bloom_slow(),
        status_code: if receipt.success { Some(U64::from(1)) } else { Some(U64::from(0)) },
        // EIP-4844 fields
        blob_gas_price: meta.excess_blob_gas.map(calc_blob_gasprice).map(U128::from),
        blob_gas_used: transaction.transaction.blob_gas_used().map(U128::from),
        ..Default::default()
    };

    #[cfg(feature = "optimism")]
    {
        let mut op_fields = OptimismTransactionReceiptFields::default();

        if transaction.is_deposit() {
            op_fields.deposit_nonce = receipt.deposit_nonce.map(U64::from);
            op_fields.deposit_receipt_version = receipt.deposit_receipt_version.map(U64::from);
        } else if let Some(l1_block_info) = optimism_tx_meta.l1_block_info {
            op_fields.l1_fee = optimism_tx_meta.l1_fee;
            op_fields.l1_gas_used = optimism_tx_meta
                .l1_data_gas
                .map(|dg| dg + l1_block_info.l1_fee_overhead.unwrap_or_default());
            op_fields.l1_fee_scalar =
                Some(f64::from(l1_block_info.l1_base_fee_scalar) / 1_000_000.0);
            op_fields.l1_gas_price = Some(l1_block_info.l1_base_fee);
        }

        res_receipt.other = op_fields.into();
    }

    match transaction.transaction.kind() {
        Create => {
            res_receipt.contract_address = Some(from.create(transaction.transaction.nonce()));
        }
        Call(addr) => {
            res_receipt.to = Some(*addr);
        }
    }

    // get number of logs in the block
    let mut num_logs = 0;
    for prev_receipt in all_receipts.iter().take(meta.index as usize) {
        num_logs += prev_receipt.logs.len();
    }

    for (tx_log_idx, log) in receipt.logs.into_iter().enumerate() {
        let rpclog = Log {
            address: log.address,
            topics: log.topics,
            data: log.data,
            block_hash: Some(meta.block_hash),
            block_number: Some(U256::from(meta.block_number)),
            transaction_hash: Some(meta.tx_hash),
            transaction_index: Some(U256::from(meta.index)),
            log_index: Some(U256::from(num_logs + tx_log_idx)),
            removed: false,
        };
        res_receipt.logs.push(rpclog);
    }

    Ok(res_receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        eth::{
            cache::EthStateCache, gas_oracle::GasPriceOracle, FeeHistoryCache,
            FeeHistoryCacheConfig,
        },
        BlockingTaskPool,
    };
    use reth_network_api::noop::NoopNetwork;
    use reth_node_ethereum::EthEvmConfig;
    use reth_primitives::{constants::ETHEREUM_BLOCK_GAS_LIMIT, hex_literal::hex};
    use reth_provider::test_utils::NoopProvider;
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
