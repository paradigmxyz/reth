//! Database access for `eth_` transaction RPC methods. Loads transaction and receipt data w.r.t.
//! network.

use alloy_dyn_abi::TypedData;
use alloy_eips::eip2718::Encodable2718;
use alloy_network::TransactionBuilder;
use alloy_primitives::{Address, Bytes, TxHash, B256};
use alloy_rpc_types::{BlockNumberOrTag, TransactionInfo};
use alloy_rpc_types_eth::transaction::TransactionRequest;
use futures::Future;
use reth_primitives::{
    BlockId, Receipt, SealedBlockWithSenders, TransactionMeta, TransactionSigned,
};
use reth_provider::{BlockNumReader, BlockReaderIdExt, ReceiptProvider, TransactionsProvider};
use reth_rpc_eth_types::{
    utils::{binary_search, recover_raw_transaction},
    EthApiError, EthStateCache, SignError, TransactionSource,
};
use reth_rpc_types_compat::transaction::{from_recovered, from_recovered_with_block_context};
use reth_transaction_pool::{PoolTransaction, TransactionOrigin, TransactionPool};

use crate::{FromEthApiError, FullEthApiTypes, IntoEthApiError, RpcReceipt, RpcTransaction};

use super::{
    Call, EthApiSpec, EthSigner, LoadBlock, LoadFee, LoadPendingBlock, LoadReceipt, LoadState,
    SpawnBlocking,
};

/// Transaction related functions for the [`EthApiServer`](crate::EthApiServer) trait in
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
/// __disable__ the base fee check in the
/// [`EnvWithHandlerCfg`](revm_primitives::CfgEnvWithHandlerCfg).
///
/// The behaviour for tracing endpoints is not consistent across clients.
/// Geth also disables the basefee check for tracing: <https://github.com/ethereum/go-ethereum/blob/bc0b87ca196f92e5af49bd33cc190ef0ec32b197/eth/tracers/api.go#L955-L955>
/// Erigon does not: <https://github.com/ledgerwatch/erigon/blob/aefb97b07d1c4fd32a66097a24eddd8f6ccacae0/turbo/transactions/tracing.go#L209-L209>
///
/// See also <https://github.com/paradigmxyz/reth/issues/6240>
///
/// This implementation follows the behaviour of Geth and disables the basefee check for tracing.
pub trait EthTransactions: LoadTransaction {
    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> impl BlockReaderIdExt;

    /// Returns a handle for signing data.
    ///
    /// Singer access in default (L1) trait method implementations.
    fn signers(&self) -> &parking_lot::RwLock<Vec<Box<dyn EthSigner>>>;

    /// Returns the transaction by hash.
    ///
    /// Checks the pool and state.
    ///
    /// Returns `Ok(None)` if no matching transaction was found.
    fn transaction_by_hash(
        &self,
        hash: B256,
    ) -> impl Future<Output = Result<Option<TransactionSource>, Self::Error>> + Send {
        LoadTransaction::transaction_by_hash(self, hash)
    }

    /// Get all transactions in the block with the given hash.
    ///
    /// Returns `None` if block does not exist.
    fn transactions_by_block(
        &self,
        block: B256,
    ) -> impl Future<Output = Result<Option<Vec<TransactionSigned>>, Self::Error>> + Send {
        async move {
            self.cache().get_block_transactions(block).await.map_err(Self::Error::from_eth_err)
        }
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
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send {
        async move {
            // Note: this is mostly used to fetch pooled transactions so we check the pool first
            if let Some(tx) =
                self.pool().get_pooled_transaction_element(hash).map(|tx| tx.envelope_encoded())
            {
                return Ok(Some(tx))
            }

            self.spawn_blocking_io(move |ref this| {
                Ok(LoadTransaction::provider(this)
                    .transaction_by_hash(hash)
                    .map_err(Self::Error::from_eth_err)?
                    .map(|tx| tx.encoded_2718().into()))
            })
            .await
        }
    }

    /// Returns the _historical_ transaction and the block it was mined in
    fn historical_transaction_by_hash_at(
        &self,
        hash: B256,
    ) -> impl Future<Output = Result<Option<(TransactionSource, B256)>, Self::Error>> + Send {
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
    ) -> impl Future<Output = Result<Option<RpcReceipt<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: LoadReceipt + 'static,
    {
        async move {
            match self.load_transaction_and_receipt(hash).await? {
                Some((tx, meta, receipt)) => {
                    self.build_transaction_receipt(tx, meta, receipt).await.map(Some)
                }
                None => Ok(None),
            }
        }
    }

    /// Helper method that loads a transaction and its receipt.
    fn load_transaction_and_receipt(
        &self,
        hash: TxHash,
    ) -> impl Future<
        Output = Result<Option<(TransactionSigned, TransactionMeta, Receipt)>, Self::Error>,
    > + Send
    where
        Self: 'static,
    {
        let this = self.clone();
        self.spawn_blocking_io(move |_| {
            let (tx, meta) = match LoadTransaction::provider(&this)
                .transaction_by_hash_with_meta(hash)
                .map_err(Self::Error::from_eth_err)?
            {
                Some((tx, meta)) => (tx, meta),
                None => return Ok(None),
            };

            let receipt = match EthTransactions::provider(&this)
                .receipt_by_hash(hash)
                .map_err(Self::Error::from_eth_err)?
            {
                Some(recpt) => recpt,
                None => return Ok(None),
            };

            Ok(Some((tx, meta, receipt)))
        })
    }

    /// Get transaction by [`BlockId`] and index of transaction within that block.
    ///
    /// Returns `Ok(None)` if the block does not exist, or index is out of range.
    fn transaction_by_block_and_tx_index(
        &self,
        block_id: BlockId,
        index: usize,
    ) -> impl Future<Output = Result<Option<RpcTransaction<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: LoadBlock,
    {
        async move {
            if let Some(block) = self.block_with_senders(block_id).await? {
                let block_hash = block.hash();
                let block_number = block.number;
                let base_fee_per_gas = block.base_fee_per_gas;
                if let Some(tx) = block.into_transactions_ecrecovered().nth(index) {
                    let tx_info = TransactionInfo {
                        hash: Some(tx.hash()),
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        base_fee: base_fee_per_gas.map(u128::from),
                        index: Some(index as u64),
                    };

                    return Ok(Some(from_recovered_with_block_context::<Self::TransactionCompat>(
                        tx, tx_info,
                    )))
                }
            }

            Ok(None)
        }
    }

    /// Find a transaction by sender's address and nonce.
    fn get_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
        include_pending: bool,
    ) -> impl Future<Output = Result<Option<RpcTransaction<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: LoadBlock + LoadState + FullEthApiTypes,
    {
        async move {
            // Check the pool first
            if include_pending {
                if let Some(tx) =
                    LoadState::pool(self).get_transaction_by_sender_and_nonce(sender, nonce)
                {
                    let transaction = tx.transaction.clone().into_consensus();
                    return Ok(Some(from_recovered::<Self::TransactionCompat>(transaction.into())));
                }
            }

            // Check if the sender is a contract
            if self.get_code(sender, None).await?.len() > 0 {
                return Ok(None);
            }

            let highest = self.transaction_count(sender, None).await?.saturating_to::<u64>();

            // If the nonce is higher or equal to the highest nonce, the transaction is pending or
            // not exists.
            if nonce >= highest {
                return Ok(None);
            }

            let Ok(high) = LoadBlock::provider(self).best_block_number() else {
                return Err(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()).into());
            };

            // Perform a binary search over the block range to find the block in which the sender's
            // nonce reached the requested nonce.
            let num = binary_search::<_, _, Self::Error>(1, high, |mid| async move {
                let mid_nonce =
                    self.transaction_count(sender, Some(mid.into())).await?.saturating_to::<u64>();

                Ok(mid_nonce > nonce)
            })
            .await?;

            let block_id = num.into();
            self.block_with_senders(block_id)
                .await?
                .and_then(|block| {
                    let block_hash = block.hash();
                    let block_number = block.number;
                    let base_fee_per_gas = block.base_fee_per_gas;

                    block
                        .into_transactions_ecrecovered()
                        .enumerate()
                        .find(|(_, tx)| tx.signer() == sender && tx.nonce() == nonce)
                        .map(|(index, tx)| {
                            let tx_info = TransactionInfo {
                                hash: Some(tx.hash()),
                                block_hash: Some(block_hash),
                                block_number: Some(block_number),
                                base_fee: base_fee_per_gas.map(u128::from),
                                index: Some(index as u64),
                            };
                            from_recovered_with_block_context::<Self::TransactionCompat>(
                                tx, tx_info,
                            )
                        })
                })
                .ok_or(EthApiError::HeaderNotFound(block_id).into())
                .map(Some)
        }
    }

    /// Get transaction, as raw bytes, by [`BlockId`] and index of transaction within that block.
    ///
    /// Returns `Ok(None)` if the block does not exist, or index is out of range.
    fn raw_transaction_by_block_and_tx_index(
        &self,
        block_id: BlockId,
        index: usize,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send
    where
        Self: LoadBlock,
    {
        async move {
            if let Some(block) = self.block_with_senders(block_id).await? {
                if let Some(tx) = block.transactions().nth(index) {
                    return Ok(Some(tx.encoded_2718().into()))
                }
            }

            Ok(None)
        }
    }

    /// Decodes and recovers the transaction and submits it to the pool.
    ///
    /// Returns the hash of the transaction.
    fn send_raw_transaction(
        &self,
        tx: Bytes,
    ) -> impl Future<Output = Result<B256, Self::Error>> + Send {
        async move {
            let recovered = recover_raw_transaction(tx.clone())?;
            let pool_transaction =
                <Self::Pool as TransactionPool>::Transaction::from_pooled(recovered.into());

            // submit the transaction to the pool with a `Local` origin
            let hash = self
                .pool()
                .add_transaction(TransactionOrigin::Local, pool_transaction)
                .await
                .map_err(Self::Error::from_eth_err)?;

            Ok(hash)
        }
    }

    /// Signs transaction with a matching signer, if any and submits the transaction to the pool.
    /// Returns the hash of the signed transaction.
    fn send_transaction(
        &self,
        mut request: TransactionRequest,
    ) -> impl Future<Output = Result<B256, Self::Error>> + Send
    where
        Self: EthApiSpec + LoadBlock + LoadPendingBlock + LoadFee + Call,
    {
        async move {
            let from = match request.from {
                Some(from) => from,
                None => return Err(SignError::NoAccount.into_eth_err()),
            };

            if self.find_signer(&from).is_err() {
                return Err(SignError::NoAccount.into_eth_err())
            }

            // set nonce if not already set before
            if request.nonce.is_none() {
                let nonce = self.transaction_count(from, Some(BlockId::pending())).await?;
                // note: `.to()` can't panic because the nonce is constructed from a `u64`
                request.nonce = Some(nonce.to());
            }

            let chain_id = self.chain_id();
            request.chain_id = Some(chain_id.to());

            let estimated_gas =
                self.estimate_gas_at(request.clone(), BlockId::pending(), None).await?;
            let gas_limit = estimated_gas;
            request.set_gas_limit(gas_limit.to());

            let signed_tx = self.sign_request(&from, request).await?;

            let recovered =
                signed_tx.into_ecrecovered().ok_or(EthApiError::InvalidTransactionSignature)?;

            let pool_transaction = <<Self as LoadTransaction>::Pool as TransactionPool>::Transaction::try_from_consensus(recovered.into()).map_err(|_| EthApiError::TransactionConversionError)?;

            // submit the transaction to the pool with a `Local` origin
            let hash = LoadTransaction::pool(self)
                .add_transaction(TransactionOrigin::Local, pool_transaction)
                .await
                .map_err(Self::Error::from_eth_err)?;

            Ok(hash)
        }
    }

    /// Signs a transaction, with configured signers.
    fn sign_request(
        &self,
        from: &Address,
        txn: TransactionRequest,
    ) -> impl Future<Output = Result<TransactionSigned, Self::Error>> + Send {
        async move {
            let signers: Vec<_> = self.signers().read().iter().cloned().collect();
            for signer in signers {
                if signer.is_signer_for(from) {
                    return match signer.sign_transaction(txn, from).await {
                        Ok(tx) => Ok(tx),
                        Err(e) => Err(e.into_eth_err()),
                    }
                }
            }
            Err(EthApiError::InvalidTransactionSignature.into())
        }
    }

    /// Signs given message. Returns the signature.
    fn sign(
        &self,
        account: Address,
        message: Bytes,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send {
        async move {
            Ok(self
                .find_signer(&account)?
                .sign(account, &message)
                .await
                .map_err(Self::Error::from_eth_err)?
                .as_bytes()
                .into())
        }
    }

    /// Encodes and signs the typed data according EIP-712. Payload must implement Eip712 trait.
    fn sign_typed_data(&self, data: &TypedData, account: Address) -> Result<Bytes, Self::Error> {
        Ok(self
            .find_signer(&account)?
            .sign_typed_data(account, data)
            .map_err(Self::Error::from_eth_err)?
            .as_bytes()
            .into())
    }

    /// Returns the signer for the given account, if found in configured signers.
    fn find_signer(
        &self,
        account: &Address,
    ) -> Result<Box<(dyn EthSigner + 'static)>, Self::Error> {
        self.signers()
            .read()
            .iter()
            .find(|signer| signer.is_signer_for(account))
            .map(|signer| dyn_clone::clone_box(&**signer))
            .ok_or_else(|| SignError::NoAccount.into_eth_err())
    }
}

/// Loads a transaction from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` transactions RPC
/// methods.
pub trait LoadTransaction: SpawnBlocking + FullEthApiTypes {
    /// Transaction pool with pending transactions. [`TransactionPool::Transaction`] is the
    /// supported transaction type.
    type Pool: TransactionPool;

    /// Returns a handle for reading data from disk.
    ///
    /// Data access in default (L1) trait method implementations.
    fn provider(&self) -> impl TransactionsProvider;

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
    ) -> impl Future<Output = Result<Option<TransactionSource>, Self::Error>> + Send {
        async move {
            // Try to find the transaction on disk
            let mut resp = self
                .spawn_blocking_io(move |this| {
                    match this
                        .provider()
                        .transaction_by_hash_with_meta(hash)
                        .map_err(Self::Error::from_eth_err)?
                    {
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
                    self.pool().get(&hash).map(|tx| tx.transaction.clone().into_consensus())
                {
                    resp = Some(TransactionSource::Pool(tx.into()));
                }
            }

            Ok(resp)
        }
    }

    /// Returns the transaction by including its corresponding [`BlockId`].
    ///
    /// Note: this supports pending transactions
    fn transaction_by_hash_at(
        &self,
        transaction_hash: B256,
    ) -> impl Future<Output = Result<Option<(TransactionSource, BlockId)>, Self::Error>> + Send
    {
        async move {
            Ok(self.transaction_by_hash(transaction_hash).await?.map(|tx| match tx {
                tx @ TransactionSource::Pool(_) => (tx, BlockId::pending()),
                tx @ TransactionSource::Block { block_hash, .. } => {
                    (tx, BlockId::Hash(block_hash.into()))
                }
            }))
        }
    }

    /// Fetches the transaction and the transaction's block
    fn transaction_and_block(
        &self,
        hash: B256,
    ) -> impl Future<Output = Result<Option<(TransactionSource, SealedBlockWithSenders)>, Self::Error>>
           + Send {
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
            let block = self
                .cache()
                .get_block_with_senders(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?;
            Ok(block.map(|block| (transaction, block.seal(block_hash))))
        }
    }
}
