//! Database access for `eth_` block RPC methods. Loads block and receipt data w.r.t. network.

use super::{LoadPendingBlock, LoadReceipt, SpawnBlocking};
use crate::{
    node::RpcNodeCoreExt, EthApiTypes, FromEthApiError, FullEthApiTypes, RpcBlock, RpcNodeCore,
    RpcReceipt,
};
use alloy_consensus::{transaction::TxHashRef, TxReceipt};
use alloy_eip7928::bal::DecodedBal;
use alloy_eips::BlockId;
use alloy_primitives::B256;
use alloy_rlp::Encodable;
use alloy_rpc_types_eth::{Block, BlockTransactions, Index};
use evm2::evm::Bal as EvmBal;
use futures::Future;
use reth_primitives_traits::{
    AlloyBlockHeader, Block as _, BlockBody, RecoveredBlock, SealedHeader, TransactionMeta,
};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert, RpcHeader};
use reth_rpc_eth_types::block::SharedReceipts;
use reth_storage_api::{
    BlockBodyIndicesProvider, BlockHashReader, BlockIdReader, BlockNumReader, BlockReader,
    HeaderProvider, ProviderHeader, ProviderReceipt, ProviderTx,
};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::sync::Arc;

/// Result type of the fetched block receipts.
pub type BlockReceiptsResult<N, E> = Result<Option<Vec<RpcReceipt<N>>>, E>;
/// Result type of the fetched block and its receipts.
pub type BlockAndReceiptsResult<Eth> = Result<
    Option<(
        Arc<RecoveredBlock<<<Eth as RpcNodeCore>::Provider as BlockReader>::Block>>,
        SharedReceipts<ProviderReceipt<<Eth as RpcNodeCore>::Provider>>,
    )>,
    <Eth as EthApiTypes>::Error,
>;

/// Block related functions for the [`EthApiServer`](crate::EthApiServer) trait in the
/// `eth_` namespace.
pub trait EthBlocks: LoadBlock<RpcConvert: RpcConvert<Primitives = Self::Primitives>> {
    /// Returns the block header for the given block id.
    fn rpc_block_header(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<RpcHeader<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: FullEthApiTypes,
    {
        async move {
            let header = match cached_block_or_hash(self, block_id).await? {
                Some(CachedBlockOrHash::Block(block)) => block.clone_sealed_header(),
                Some(CachedBlockOrHash::Hash(block_hash)) => {
                    let Some(header) = self
                        .provider()
                        .sealed_header_by_hash(block_hash)
                        .map_err(Self::Error::from_eth_err)?
                    else {
                        return Ok(None)
                    };
                    header
                }
                None => return Ok(None),
            };
            // Header responses omit the block size: https://github.com/ethereum/execution-apis/pull/877
            let header = self.converter().convert_header(header, None)?;
            Ok(Some(header))
        }
    }

    /// Returns the populated rpc block object for the given block id.
    ///
    /// If `full` is true, the block object will contain all transaction objects, otherwise it will
    /// only contain the transaction hashes.
    fn rpc_block(
        &self,
        block_id: BlockId,
        full: bool,
    ) -> impl Future<Output = Result<Option<RpcBlock<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: FullEthApiTypes,
    {
        async move {
            let Some(block) = self.recovered_block(block_id).await? else { return Ok(None) };

            let block = block.clone_into_rpc_block(
                full.into(),
                |tx, tx_info| self.converter().fill(tx, tx_info),
                |header, block_size| self.converter().convert_header(header, Some(block_size)),
            )?;
            Ok(Some(block))
        }
    }

    /// Returns the number transactions in the given block.
    ///
    /// Returns `None` if the block does not exist
    fn block_transaction_count(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<usize>, Self::Error>> + Send {
        async move {
            let block_hash = match cached_block_or_hash(self, block_id).await? {
                Some(CachedBlockOrHash::Block(block)) => {
                    return Ok(Some(block.body().transaction_count()))
                }
                Some(CachedBlockOrHash::Hash(block_hash)) => block_hash,
                None => return Ok(None),
            };

            let provider = self.provider();
            let Some(number) =
                provider.block_number(block_hash).map_err(Self::Error::from_eth_err)?
            else {
                return Ok(None)
            };
            // The body indices are read by number, so they belong to the requested block only if
            // the number still maps to its hash afterwards. If a reorg moved it, or the indices are
            // missing, the count comes from the block itself.
            if let Some(indices) =
                provider.block_body_indices(number).map_err(Self::Error::from_eth_err)? &&
                provider.block_hash(number).map_err(Self::Error::from_eth_err)? ==
                    Some(block_hash)
            {
                return Ok(Some(indices.tx_count as usize))
            }

            Ok(self
                .cache()
                .get_recovered_block(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?
                .map(|block| block.body().transaction_count()))
        }
    }

    /// Helper function for `eth_getBlockReceipts`.
    ///
    /// Returns all transaction receipts in block, or `None` if block wasn't found.
    fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = BlockReceiptsResult<Self::NetworkTypes, Self::Error>> + Send
    where
        Self: LoadReceipt,
    {
        async move {
            if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
                let block_number = block.number();
                let base_fee = block.base_fee_per_gas();
                let block_hash = block.hash();
                let excess_blob_gas = block.excess_blob_gas();
                let timestamp = block.timestamp();
                let mut gas_used = 0;
                let mut next_log_index = 0;

                let inputs = block
                    .transactions_recovered()
                    .zip(receipts.into_vec())
                    .enumerate()
                    .map(|(idx, (tx, receipt))| {
                        let meta = TransactionMeta {
                            tx_hash: *tx.tx_hash(),
                            index: idx as u64,
                            block_hash,
                            block_number,
                            base_fee,
                            excess_blob_gas,
                            timestamp,
                        };

                        let cumulative_gas_used = receipt.cumulative_gas_used();
                        let logs_len = receipt.logs().len();

                        let input = ConvertReceiptInput {
                            tx,
                            gas_used: cumulative_gas_used - gas_used,
                            next_log_index,
                            meta,
                            receipt,
                        };

                        gas_used = cumulative_gas_used;
                        next_log_index += logs_len;

                        input
                    })
                    .collect::<Vec<_>>();

                return Ok(self
                    .converter()
                    .convert_receipts_with_block(inputs, block.sealed_block())
                    .map(Some)?)
            }

            Ok(None)
        }
    }

    /// Helper method that loads a block and all its receipts.
    fn load_block_and_receipts(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = BlockAndReceiptsResult<Self>> + Send
    where
        Self: LoadReceipt,
        Self::Pool:
            TransactionPool<Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>>,
    {
        async move {
            if block_id.is_pending() {
                if self.pending_block_kind().is_none() {
                    return Ok(None);
                }

                // First, try to get the pending block from the provider, in case we already
                // received the actual pending block from the CL.
                if let Some(pending) = self
                    .provider()
                    .pending_block_and_receipts()
                    .map_err(Self::Error::from_eth_err)?
                {
                    let (block, output) = pending.into_parts();
                    return Ok(Some((block, output.into())));
                }

                // If no pending block from provider, build the pending block locally.
                if let Some(pending) = self.local_pending_block().await? {
                    return Ok(Some((pending.block, pending.receipts)));
                }
            }

            if let Some(block_hash) =
                self.provider().block_hash_for_id(block_id).map_err(Self::Error::from_eth_err)? &&
                let Some((block, receipts)) = self
                    .cache()
                    .get_block_and_receipts(block_hash)
                    .await
                    .map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some((block, receipts.into())));
            }

            Ok(None)
        }
    }

    /// Returns uncle headers of given block.
    ///
    /// Returns an empty vec if there are none.
    #[expect(clippy::type_complexity)]
    fn ommers(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<Vec<ProviderHeader<Self::Provider>>>, Self::Error>> + Send
    {
        async move {
            match cached_block_or_hash(self, block_id).await? {
                Some(CachedBlockOrHash::Block(block)) => {
                    Ok(block.body().ommers().map(|o| o.to_vec()))
                }
                // The ommers are part of the body, but reading the block without its senders
                // skips the senders table and, where senders are pruned, their recovery.
                Some(CachedBlockOrHash::Hash(block_hash)) => {
                    self.spawn_blocking_io(move |this| {
                        let block = this
                            .provider()
                            .block_by_hash(block_hash)
                            .map_err(Self::Error::from_eth_err)?;
                        Ok(block.and_then(|block| block.body().ommers().map(|o| o.to_vec())))
                    })
                    .await
                }
                None => Ok(None),
            }
        }
    }

    /// Returns uncle block at given index in given block.
    ///
    /// Returns `None` if index out of range.
    fn ommer_by_block_and_index(
        &self,
        block_id: BlockId,
        index: Index,
    ) -> impl Future<Output = Result<Option<RpcBlock<Self::NetworkTypes>>, Self::Error>> + Send
    {
        async move {
            let uncles = self.ommers(block_id).await?.unwrap_or_default();

            uncles
                .into_iter()
                .nth(index.into())
                .map(|header| {
                    let block =
                        alloy_consensus::Block::<alloy_consensus::TxEnvelope, _>::uncle(header);
                    let size = block.length();
                    let header = self
                        .converter()
                        .convert_header(SealedHeader::new_unhashed(block.header), Some(size))?;
                    Ok(Block {
                        uncles: vec![],
                        header,
                        transactions: BlockTransactions::Uncle,
                        withdrawals: None,
                    })
                })
                .transpose()
        }
    }
}

/// Loads a block from database.
///
/// Behaviour shared by several `eth_` RPC methods, not exclusive to `eth_` blocks RPC methods.
pub trait LoadBlock: LoadPendingBlock + SpawnBlocking + RpcNodeCoreExt {
    /// Returns the block object for the given block id.
    #[expect(clippy::type_complexity)]
    fn recovered_block(
        &self,
        block_id: BlockId,
    ) -> impl Future<
        Output = Result<
            Option<Arc<RecoveredBlock<<Self::Provider as BlockReader>::Block>>>,
            Self::Error,
        >,
    > + Send {
        async move {
            if block_id.is_pending() {
                if self.pending_block_kind().is_none() {
                    return Ok(None);
                }

                // Pending block can be fetched directly without need for caching
                if let Some(pending_block) =
                    self.provider().pending_block().map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some(pending_block));
                }

                // If no pending block from provider, try to get local pending block
                return match self.local_pending_block().await? {
                    Some(pending) => Ok(Some(pending.block)),
                    None => Ok(None),
                };
            }

            let block_hash = match self
                .provider()
                .block_hash_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
            {
                Some(block_hash) => block_hash,
                None => return Ok(None),
            };

            self.cache().get_recovered_block(block_hash).await.map_err(Self::Error::from_eth_err)
        }
    }

    /// Returns the block for the given block id, together with the block's cached block access
    /// list, if any.
    ///
    /// The BAL is only returned if it is already cached, it is never fetched from the BAL store.
    /// Pending blocks never have a BAL.
    #[expect(clippy::type_complexity)]
    fn recovered_block_and_maybe_bal(
        &self,
        block_id: BlockId,
    ) -> impl Future<
        Output = Result<
            Option<(
                Arc<RecoveredBlock<<Self::Provider as BlockReader>::Block>>,
                Option<Arc<DecodedBal<Arc<EvmBal>>>>,
            )>,
            Self::Error,
        >,
    > + Send {
        async move {
            if block_id.is_pending() {
                return Ok(self.recovered_block(block_id).await?.map(|block| (block, None)));
            }

            let block_hash = match self
                .provider()
                .block_hash_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
            {
                Some(block_hash) => block_hash,
                None => return Ok(None),
            };

            self.cache()
                .get_recovered_block_and_maybe_bal(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)
        }
    }
}

/// A block found without loading it into the cache, or the hash to read it by.
enum CachedBlockOrHash<B: reth_primitives_traits::Block> {
    /// The pending block or a block that is already cached.
    Block(Arc<RecoveredBlock<B>>),
    /// The hash of a block that is not cached.
    Hash(B256),
}

/// Returns the pending block or the cached block for the given block id, or the hash of the block
/// if it is not cached.
///
/// Unlike [`LoadBlock::recovered_block`], this never loads an uncached block with its senders, so
/// a request that needs only part of the block can read that part from the provider.
///
/// Returns `None` if the pending block is not available or the block id does not resolve to a
/// hash.
async fn cached_block_or_hash<T: LoadBlock>(
    eth: &T,
    block_id: BlockId,
) -> Result<Option<CachedBlockOrHash<<T::Provider as BlockReader>::Block>>, T::Error> {
    if block_id.is_pending() {
        return Ok(eth.recovered_block(block_id).await?.map(CachedBlockOrHash::Block))
    }

    let Some(block_hash) =
        eth.provider().block_hash_for_id(block_id).map_err(T::Error::from_eth_err)?
    else {
        return Ok(None)
    };

    Ok(Some(
        match eth.cache().get_maybe_block(block_hash).await.map_err(T::Error::from_eth_err)? {
            Some(block) => CachedBlockOrHash::Block(block),
            None => CachedBlockOrHash::Hash(block_hash),
        },
    ))
}
