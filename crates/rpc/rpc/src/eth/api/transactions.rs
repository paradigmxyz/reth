//! Contains RPC handler implementations specific to transactions
use crate::{
    eth::{
        error::{EthApiError, EthResult, SignError},
        revm_utils::{inspect, prepare_call_env, replay_transactions_until, transact},
        utils::recover_raw_transaction,
    },
    EthApi, EthApiSpec,
};
use async_trait::async_trait;

use reth_network_api::NetworkInfo;
use reth_primitives::{
    Address, BlockId, BlockNumberOrTag, Bytes, FromRecoveredTransaction, Header,
    IntoRecoveredTransaction, Receipt, SealedBlock,
    TransactionKind::{Call, Create},
    TransactionMeta, TransactionSigned, TransactionSignedEcRecovered, H256, U128, U256, U64,
};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderBox, StateProviderFactory};
use reth_revm::{
    database::{State, SubState},
    env::{fill_block_env_with_coinbase, tx_env_with_recovered},
    tracing::{TracingInspector, TracingInspectorConfig},
};
use reth_rpc_types::{
    state::StateOverride, CallRequest, Index, Log, Transaction, TransactionInfo,
    TransactionReceipt, TransactionRequest, TypedTransactionRequest,
};
use reth_transaction_pool::{TransactionOrigin, TransactionPool};
use revm::{
    db::CacheDB,
    primitives::{BlockEnv, CfgEnv},
    Inspector,
};
use revm_primitives::{utilities::create_address, Env, ResultAndState, SpecId};

/// Commonly used transaction related functions for the [EthApi] type in the `eth_` namespace
#[async_trait::async_trait]
pub trait EthTransactions: Send + Sync {
    /// Returns the state at the given [BlockId]
    fn state_at(&self, at: BlockId) -> EthResult<StateProviderBox<'_>>;

    /// Executes the closure with the state that corresponds to the given [BlockId].
    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox<'_>) -> EthResult<T>;

    /// Returns the revm evm env for the requested [BlockId]
    ///
    /// If the [BlockId] this will return the [BlockId::Hash] of the block the env was configured
    /// for.
    async fn evm_env_at(&self, at: BlockId) -> EthResult<(CfgEnv, BlockEnv, BlockId)>;

    /// Returns the revm evm env for the raw block header
    ///
    /// This is used for tracing raw blocks
    async fn evm_env_for_raw_block(&self, at: &Header) -> EthResult<(CfgEnv, BlockEnv)>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block(&self, block: H256)
        -> EthResult<Option<Vec<TransactionSigned>>>;

    /// Get the entire block for the given id.
    ///
    /// Returns `None` if block does not exist.
    async fn block_by_id(&self, id: BlockId) -> EthResult<Option<SealedBlock>>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    async fn transactions_by_block_id(
        &self,
        block: BlockId,
    ) -> EthResult<Option<Vec<TransactionSigned>>>;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    async fn transaction_by_hash(&self, hash: H256) -> EthResult<Option<TransactionSource>>;

    /// Returns the transaction by including its corresponding [BlockId]
    ///
    /// Note: this supports pending transactions
    async fn transaction_by_hash_at(
        &self,
        hash: H256,
    ) -> EthResult<Option<(TransactionSource, BlockId)>>;

    /// Returns the _historical_ transaction and the block it was mined in
    async fn historical_transaction_by_hash_at(
        &self,
        hash: H256,
    ) -> EthResult<Option<(TransactionSource, H256)>>;

    /// Returns the transaction receipt for the given hash.
    ///
    /// Returns None if the transaction does not exist or is pending
    /// Note: The tx receipt is not available for pending transactions.
    async fn transaction_receipt(&self, hash: H256) -> EthResult<Option<TransactionReceipt>>;

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<H256>;

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    async fn send_transaction(&self, request: TransactionRequest) -> EthResult<H256>;

    /// Prepares the state and env for the given [CallRequest] at the given [BlockId] and executes
    /// the closure.
    async fn with_call_at<F, R>(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
        f: F,
    ) -> EthResult<R>
    where
        F: for<'r> FnOnce(CacheDB<State<StateProviderBox<'r>>>, Env) -> EthResult<R> + Send;

    /// Executes the call request at the given [BlockId].
    async fn transact_call_at(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
    ) -> EthResult<(ResultAndState, Env)>;

    /// Executes the call request at the given [BlockId]
    async fn inspect_call_at<I>(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
        inspector: I,
    ) -> EthResult<(ResultAndState, Env)>
    where
        I: for<'r> Inspector<CacheDB<State<StateProviderBox<'r>>>> + Send;

    /// Executes the transaction on top of the given [BlockId] with a tracer configured by the
    /// config.
    ///
    /// The callback is then called with the [TracingInspector] and the [ResultAndState] after the
    /// configured [Env] was inspected.
    fn trace_at<F, R>(
        &self,
        env: Env,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>;

    /// Fetches the transaction and the transaction's block
    async fn transaction_and_block(
        &self,
        hash: H256,
    ) -> EthResult<Option<(TransactionSource, SealedBlock)>>;

    /// Retrieves the transaction if it exists and returns its trace.
    ///
    /// Before the transaction is traced, all previous transaction in the block are applied to the
    /// state by executing them first
    async fn trace_transaction_in_block<F, R>(
        &self,
        hash: H256,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<R>>
    where
        F: FnOnce(TransactionInfo, TracingInspector, ResultAndState) -> EthResult<R> + Send;
}

#[async_trait]
impl<Client, Pool, Network> EthTransactions for EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + Send + Sync + 'static,
{
    fn state_at(&self, at: BlockId) -> EthResult<StateProviderBox<'_>> {
        self.state_at_block_id(at)
    }

    fn with_state_at_block<F, T>(&self, at: BlockId, f: F) -> EthResult<T>
    where
        F: FnOnce(StateProviderBox<'_>) -> EthResult<T>,
    {
        let state = self.state_at(at)?;
        f(state)
    }

    async fn evm_env_at(&self, at: BlockId) -> EthResult<(CfgEnv, BlockEnv, BlockId)> {
        // TODO handle Pending state's env
        match at {
            BlockId::Number(BlockNumberOrTag::Pending) => {
                // This should perhaps use the latest env settings and update block specific
                // settings like basefee/number
                Err(EthApiError::Unsupported("pending state not implemented yet"))
            }
            hash_or_num => {
                let block_hash = self
                    .client()
                    .block_hash_for_id(hash_or_num)?
                    .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
                let (cfg, env) = self.cache().get_evm_env(block_hash).await?;
                Ok((cfg, env, block_hash.into()))
            }
        }
    }

    async fn evm_env_for_raw_block(&self, header: &Header) -> EthResult<(CfgEnv, BlockEnv)> {
        // get the parent config first
        let (cfg, mut block_env, _) = self.evm_env_at(header.parent_hash.into()).await?;

        let after_merge = cfg.spec_id >= SpecId::MERGE;
        fill_block_env_with_coinbase(&mut block_env, header, after_merge, header.beneficiary);

        Ok((cfg, block_env))
    }

    async fn transactions_by_block(
        &self,
        block: H256,
    ) -> EthResult<Option<Vec<TransactionSigned>>> {
        Ok(self.cache().get_block_transactions(block).await?)
    }

    async fn block_by_id(&self, id: BlockId) -> EthResult<Option<SealedBlock>> {
        self.block(id).await
    }

    async fn transactions_by_block_id(
        &self,
        block: BlockId,
    ) -> EthResult<Option<Vec<TransactionSigned>>> {
        self.block_by_id(block).await.map(|block| block.map(|block| block.body))
    }

    async fn transaction_by_hash(&self, hash: H256) -> EthResult<Option<TransactionSource>> {
        if let Some(tx) = self.pool().get(&hash).map(|tx| tx.transaction.to_recovered_transaction())
        {
            return Ok(Some(TransactionSource::Pool(tx)))
        }

        match self.client().transaction_by_hash_with_meta(hash)? {
            None => Ok(None),
            Some((tx, meta)) => {
                let transaction =
                    tx.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

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
    }

    async fn transaction_by_hash_at(
        &self,
        transaction_hash: H256,
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
        hash: H256,
    ) -> EthResult<Option<(TransactionSource, H256)>> {
        match self.transaction_by_hash_at(hash).await? {
            None => Ok(None),
            Some((tx, at)) => Ok(at.as_block_hash().map(|hash| (tx, hash))),
        }
    }

    async fn transaction_receipt(&self, hash: H256) -> EthResult<Option<TransactionReceipt>> {
        let (tx, meta) = match self.client().transaction_by_hash_with_meta(hash)? {
            Some((tx, meta)) => (tx, meta),
            None => return Ok(None),
        };

        let receipt = match self.client().receipt_by_hash(hash)? {
            Some(recpt) => recpt,
            None => return Ok(None),
        };

        self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
    }

    async fn send_raw_transaction(&self, tx: Bytes) -> EthResult<H256> {
        let recovered = recover_raw_transaction(tx)?;

        let pool_transaction = <Pool::Transaction>::from_recovered_transaction(recovered);

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }

    async fn send_transaction(&self, mut request: TransactionRequest) -> EthResult<H256> {
        let from = match request.from {
            Some(from) => from,
            None => return Err(SignError::NoAccount.into()),
        };

        // set nonce if not already set before
        if request.nonce.is_none() {
            let nonce =
                self.get_transaction_count(from, Some(BlockId::Number(BlockNumberOrTag::Pending)))?;
            request.nonce = Some(nonce);
        }

        let chain_id = self.chain_id();
        // TODO: we need an oracle to fetch the gas price of the current chain
        let gas_price = request.gas_price.unwrap_or_default();
        let max_fee_per_gas = request.max_fee_per_gas.unwrap_or_default();

        let estimated_gas = self
            .estimate_gas_at(
                CallRequest {
                    from: Some(from),
                    to: request.to,
                    gas: request.gas,
                    gas_price: Some(U256::from(gas_price)),
                    max_fee_per_gas: Some(U256::from(max_fee_per_gas)),
                    value: request.value,
                    data: request.data.clone(),
                    nonce: request.nonce,
                    chain_id: Some(chain_id),
                    access_list: request.access_list.clone(),
                    max_priority_fee_per_gas: Some(U256::from(max_fee_per_gas)),
                    transaction_type: None,
                },
                BlockId::Number(BlockNumberOrTag::Pending),
            )
            .await?;
        let gas_limit = estimated_gas;

        let transaction = match request.into_typed_request() {
            Some(TypedTransactionRequest::Legacy(mut m)) => {
                m.chain_id = Some(chain_id.as_u64());
                m.gas_limit = gas_limit;
                m.gas_price = gas_price;

                TypedTransactionRequest::Legacy(m)
            }
            Some(TypedTransactionRequest::EIP2930(mut m)) => {
                m.chain_id = chain_id.as_u64();
                m.gas_limit = gas_limit;
                m.gas_price = gas_price;

                TypedTransactionRequest::EIP2930(m)
            }
            Some(TypedTransactionRequest::EIP1559(mut m)) => {
                m.chain_id = chain_id.as_u64();
                m.gas_limit = gas_limit;
                m.max_fee_per_gas = max_fee_per_gas;

                TypedTransactionRequest::EIP1559(m)
            }
            None => return Err(EthApiError::ConflictingFeeFieldsInRequest),
        };

        let signed_tx = self.sign_request(&from, transaction)?;

        let recovered =
            signed_tx.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

        let pool_transaction = <Pool::Transaction>::from_recovered_transaction(recovered);

        // submit the transaction to the pool with a `Local` origin
        let hash = self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

        Ok(hash)
    }

    async fn with_call_at<F, R>(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
        f: F,
    ) -> EthResult<R>
    where
        F: for<'r> FnOnce(CacheDB<State<StateProviderBox<'r>>>, Env) -> EthResult<R> + Send,
    {
        let (cfg, block_env, at) = self.evm_env_at(at).await?;
        let state = self.state_at(at)?;
        let mut db = SubState::new(State::new(state));

        let env = prepare_call_env(cfg, block_env, request, &mut db, state_overrides)?;
        f(db, env)
    }

    async fn transact_call_at(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
    ) -> EthResult<(ResultAndState, Env)> {
        self.with_call_at(request, at, state_overrides, |mut db, env| transact(&mut db, env)).await
    }

    async fn inspect_call_at<I>(
        &self,
        request: CallRequest,
        at: BlockId,
        state_overrides: Option<StateOverride>,
        inspector: I,
    ) -> EthResult<(ResultAndState, Env)>
    where
        I: for<'r> Inspector<CacheDB<State<StateProviderBox<'r>>>> + Send,
    {
        self.with_call_at(request, at, state_overrides, |db, env| inspect(db, env, inspector)).await
    }

    fn trace_at<F, R>(
        &self,
        env: Env,
        config: TracingInspectorConfig,
        at: BlockId,
        f: F,
    ) -> EthResult<R>
    where
        F: FnOnce(TracingInspector, ResultAndState) -> EthResult<R>,
    {
        self.with_state_at_block(at, |state| {
            let db = SubState::new(State::new(state));

            let mut inspector = TracingInspector::new(config);
            let (res, _) = inspect(db, env, &mut inspector)?;

            f(inspector, res)
        })
    }

    async fn transaction_and_block(
        &self,
        hash: H256,
    ) -> EthResult<Option<(TransactionSource, SealedBlock)>> {
        let (transaction, at) = match self.transaction_by_hash_at(hash).await? {
            None => return Ok(None),
            Some(res) => res,
        };

        // Note: this is always either hash or pending
        let block_hash = match at {
            BlockId::Hash(hash) => hash.block_hash,
            _ => return Ok(None),
        };
        let block = self.cache().get_block(block_hash).await?;
        Ok(block.map(|block| (transaction, block.seal(block_hash))))
    }

    async fn trace_transaction_in_block<F, R>(
        &self,
        hash: H256,
        config: TracingInspectorConfig,
        f: F,
    ) -> EthResult<Option<R>>
    where
        F: FnOnce(TransactionInfo, TracingInspector, ResultAndState) -> EthResult<R> + Send,
    {
        let (transaction, block) = match self.transaction_and_block(hash).await? {
            None => return Ok(None),
            Some(res) => res,
        };
        let (tx, tx_info) = transaction.split();

        let (cfg, block_env, _) = self.evm_env_at(block.hash.into()).await?;

        // we need to get the state of the parent block because we're essentially replaying the
        // block the transaction is included in
        let parent_block = block.parent_hash;
        let block_txs = block.body;

        self.with_state_at_block(parent_block.into(), |state| {
            let mut db = SubState::new(State::new(state));

            // replay all transactions prior to the targeted transaction
            replay_transactions_until(&mut db, cfg.clone(), block_env.clone(), block_txs, tx.hash)?;

            let env = Env { cfg, block: block_env, tx: tx_env_with_recovered(&tx) };

            let mut inspector = TracingInspector::new(config);
            let (res, _) = inspect(db, env, &mut inspector)?;
            f(tx_info, inspector, res)
        })
        .map(Some)
    }
}

// === impl EthApi ===

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: 'static,
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
        let block_id = block_id.into();

        if let Some(block) = self.block(block_id).await? {
            let block_hash = block.hash;
            let block = block.unseal();
            if let Some(tx_signed) = block.body.into_iter().nth(index.into()) {
                let tx =
                    tx_signed.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;
                return Ok(Some(Transaction::from_recovered_with_block_context(
                    tx,
                    block_hash,
                    block.header.number,
                    block.header.base_fee_per_gas,
                    index.into(),
                )))
            }
        }

        Ok(None)
    }

    /// Helper function for `eth_getTransactionReceipt`
    ///
    /// Returns the receipt
    pub(crate) async fn build_transaction_receipt(
        &self,
        tx: TransactionSigned,
        meta: TransactionMeta,
        receipt: Receipt,
    ) -> EthResult<TransactionReceipt> {
        let transaction =
            tx.clone().into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

        // get all receipts for the block
        let all_receipts = match self.cache().get_receipts(meta.block_hash).await? {
            Some(recpts) => recpts,
            None => return Err(EthApiError::UnknownBlockNumber),
        };

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

        let mut res_receipt = TransactionReceipt {
            transaction_hash: Some(meta.tx_hash),
            transaction_index: Some(U256::from(meta.index)),
            block_hash: Some(meta.block_hash),
            block_number: Some(U256::from(meta.block_number)),
            from: transaction.signer(),
            to: None,
            cumulative_gas_used: U256::from(receipt.cumulative_gas_used),
            gas_used: Some(U256::from(gas_used)),
            contract_address: None,
            logs: Vec::with_capacity(receipt.logs.len()),
            effective_gas_price: U128::from(transaction.effective_gas_price(meta.base_fee)),
            transaction_type: tx.transaction.tx_type().into(),
            // TODO pre-byzantium receipts have a post-transaction state root
            state_root: None,
            logs_bloom: receipt.bloom_slow(),
            status_code: if receipt.success { Some(U64::from(1)) } else { Some(U64::from(0)) },
        };

        match tx.transaction.kind() {
            Create => {
                res_receipt.contract_address =
                    Some(create_address(transaction.signer(), tx.transaction.nonce()));
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
        block_hash: H256,
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
            TransactionSource::Pool(tx) => Transaction::from_recovered(tx),
            TransactionSource::Block { transaction, index, block_hash, block_number, base_fee } => {
                Transaction::from_recovered_with_block_context(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eth::cache::EthStateCache, EthApi};
    use reth_network_api::test_utils::NoopNetwork;
    use reth_primitives::{hex_literal::hex, Bytes};
    use reth_provider::test_utils::NoopProvider;
    use reth_transaction_pool::{test_utils::testing_pool, TransactionPool};

    #[tokio::test]
    async fn send_raw_transaction() {
        let noop_provider = NoopProvider::default();
        let noop_network_provider = NoopNetwork;

        let pool = testing_pool();

        let eth_api = EthApi::new(
            noop_provider,
            pool.clone(),
            noop_network_provider,
            EthStateCache::spawn(NoopProvider::default(), Default::default()),
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
