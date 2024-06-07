//! Database access for `eth_` transaction RPC methods. Loads transaction and receipt data w.r.t.
//! network.

use std::sync::Arc;

use futures::Future;
use reth_primitives::{
    BlockId, Bytes, FromRecoveredPooledTransaction, IntoRecoveredTransaction, Receipt,
    SealedBlockWithSenders, TransactionMeta, TransactionSigned, TxHash, B256,
};
use reth_provider::{BlockReaderIdExt, ReceiptProvider, TransactionsProvider};
use reth_rpc_types::{AnyTransactionReceipt, TransactionRequest};
use reth_transaction_pool::{TransactionOrigin, TransactionPool};

use crate::eth::{
    api::{BuildReceipt, Call, SpawnBlocking},
    cache::EthStateCache,
    error::{EthApiError, EthResult},
    traits::RawTransactionForwarder,
    utils::recover_raw_transaction,
    TransactionSource,
};

/// Transaction related functions for the [`EthApiServer`](crate::EthApi) trait in
/// the `eth_` namespace.
///
/// This includes utilities for transaction tracing, transacting and inspection.
///
/// Async functions that are spawned onto the
/// [`BlockingTaskPool`](reth_tasks::pool::BlockingTaskPool) begin with `spawn_`
///
/// ## Calls
///
/// There are subtle differences between when transacting [`TransactionRequest`]:
///
/// The endpoints `eth_call` and `eth_estimateGas` and `eth_createAccessList` should always
/// __disable__ the base fee check in the [`EnvWithHandlerCfg`]
/// [`Cfg`](revm_primitives::CfgEnvWithHandlerCfg).
///
/// The behaviour for tracing endpoints is not consistent across clients.
/// Geth also disables the basefee check for tracing: <https://github.com/ethereum/go-ethereum/blob/bc0b87ca196f92e5af49bd33cc190ef0ec32b197/eth/tracers/api.go#L955-L955>
/// Erigon does not: <https://github.com/ledgerwatch/erigon/blob/aefb97b07d1c4fd32a66097a24eddd8f6ccacae0/turbo/transactions/tracing.go#L209-L209>
///
/// See also <https://github.com/paradigmxyz/reth/issues/6240>
///
/// This implementation follows the behaviour of Geth and disables the basefee check for tracing.
pub trait EthTransactions: LoadTransaction + Send + Sync {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> &impl BlockReaderIdExt;

    /// Returns a handle for forwarding received raw transactions.
    ///
    /// Access to transaction forwarder in default (L1) trait method implementations.
    fn raw_tx_forwarder(&self) -> &Option<Arc<dyn RawTransactionForwarder>>;

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    fn transactions_by_block(
        &self,
        block: B256,
    ) -> impl Future<Output = EthResult<Option<Vec<TransactionSigned>>>> + Send {
        async move { Ok(self.cache().get_block_transactions(block).await?) }
    }

    /// Returns the EIP-2718 encoded transaction by hash.
    ///
    /// If this is a pooled EIP-4844 transaction, the blob sidecar is included.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    fn raw_transaction_by_hash(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<Bytes>>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            // Note: this is mostly used to fetch pooled transactions so we check the pool first
            if let Some(tx) =
                self.pool().get_pooled_transaction_element(hash).map(|tx| tx.envelope_encoded())
            {
                return Ok(Some(tx))
            }

            self.spawn_blocking_io(move |ref this| {
                Ok(LoadTransaction::provider(this)
                    .transaction_by_hash(hash)?
                    .map(|tx| tx.envelope_encoded()))
            })
            .await
        }
    }

    /// Returns the _historical_ transaction and the block it was mined in
    fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<(TransactionSource, B256)>>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            match self.transaction_by_hash_at(hash).await? {
                None => Ok(None),
                Some((tx, at)) => Ok(at.as_block_hash().map(|hash| (tx, hash))),
            }
        }
    }

    /// Returns the transaction receipt for the given hash.
    ///
    /// Returns None if the transaction does not exist or is pending
    /// Note: The tx receipt is not available for pending transactions.
    fn transaction_receipt(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<AnyTransactionReceipt>>> + Send
    where
        Self: BuildReceipt + SpawnBlocking + Clone + 'static,
    {
        async move {
            let result = self.load_transaction_and_receipt(hash).await?;

            let (tx, meta, receipt) = match result {
                Some((tx, meta, receipt)) => (tx, meta, receipt),
                None => return Ok(None),
            };

            self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
        }
    }

    /// Helper method that loads a transaction and its receipt.
    fn load_transaction_and_receipt(
        &self,
        hash: TxHash,
    ) -> impl Future<Output = EthResult<Option<(TransactionSigned, TransactionMeta, Receipt)>>> + Send
    where
        Self: SpawnBlocking + Clone + 'static,
    {
        let this = self.clone();
        self.spawn_blocking_io(move |_| {
            let (tx, meta) =
                match LoadTransaction::provider(&this).transaction_by_hash_with_meta(hash)? {
                    Some((tx, meta)) => (tx, meta),
                    None => return Ok(None),
                };

            let receipt = match EthTransactions::provider(&this).receipt_by_hash(hash)? {
                Some(recpt) => recpt,
                None => return Ok(None),
            };

            Ok(Some((tx, meta, receipt)))
        })
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    fn send_raw_transaction(&self, tx: Bytes) -> impl Future<Output = EthResult<B256>> + Send {
        async move {
            // On optimism, transactions are forwarded directly to the sequencer to be included in
            // blocks that it builds.
            if let Some(client) = self.raw_tx_forwarder().as_ref() {
                tracing::debug!( target: "rpc::eth",  "forwarding raw transaction to");
                client.forward_raw_transaction(&tx).await?;
            }

            let recovered = recover_raw_transaction(tx)?;
            let pool_transaction =
                <Self::Pool as TransactionPool>::Transaction::from_recovered_pooled_transaction(
                    recovered,
                );

            // submit the transaction to the pool with a `Local` origin
            let hash =
                self.pool().add_transaction(TransactionOrigin::Local, pool_transaction).await?;

            Ok(hash)
        }
    }

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    fn send_transaction(
        &self,
        request: TransactionRequest,
    ) -> impl Future<Output = EthResult<B256>> + Send
    where
        Self: Call;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    fn transaction_by_hash(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<TransactionSource>>> + Send
    where
        Self: SpawnBlocking,
    {
        LoadTransaction::transaction_by_hash(self, hash)
    }
}

pub trait LoadTransaction {
    /// Transaction pool with pending transactions. [`TransactionPool::Transaction`] is the
    /// supported transaction type.
    type Pool: TransactionPool;

    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> &impl TransactionsProvider;

    /// Returns a handle for reading data from memory.
    ///
    /// Data access in default (L1) trait method implementations.
    fn cache(&self) -> &EthStateCache;

    /// Returns a handle for reading data from pool.
    ///
    /// Data access in default (L1) trait method implementations.
    fn pool(&self) -> &Self::Pool;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    fn transaction_by_hash(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<TransactionSource>>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            // Try to find the transaction on disk
            let mut resp = self
                .spawn_blocking_io(move |this| {
                    match this.provider().transaction_by_hash_with_meta(hash)? {
                        None => Ok(None),
                        Some((tx, meta)) => {
                            // Note: we assume this transaction is valid, because it's mined (or
                            // part of pending block) and already. We don't need to
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
    }

    /// Returns the transaction by including its corresponding [BlockId]
    ///
    /// Note: this supports pending transactions
    fn transaction_by_hash_at(
        &self,
        transaction_hash: B256,
    ) -> impl Future<Output = EthResult<Option<(TransactionSource, BlockId)>>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
            match self.transaction_by_hash(transaction_hash).await? {
                None => return Ok(None),
                Some(tx) => {
                    let res = match tx {
                        tx @ TransactionSource::Pool(_) => (tx, BlockId::pending()),
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
    }

    /// Fetches the transaction and the transaction's block
    fn transaction_and_block(
        &self,
        hash: B256,
    ) -> impl Future<Output = EthResult<Option<(TransactionSource, SealedBlockWithSenders)>>> + Send
    where
        Self: SpawnBlocking,
    {
        async move {
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
    }
}
