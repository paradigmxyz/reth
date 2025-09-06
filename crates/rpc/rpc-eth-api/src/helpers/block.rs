//! Database access for `eth_` block RPC methods. Loads block and receipt data w.r.t. network.

use super::{LoadPendingBlock, LoadReceipt, SpawnBlocking};
use crate::{
    node::RpcNodeCoreExt, EthApiTypes, FromEthApiError, FullEthApiTypes, RpcBlock, RpcNodeCore,
    RpcReceipt,
};
use alloy_consensus::TxReceipt;
use alloy_eips::BlockId;
use alloy_rlp::Encodable;
use alloy_rpc_types_eth::{Block, BlockTransactions, Index};
use futures::Future;
use reth_node_api::BlockBody;
use reth_primitives_traits::{
    AlloyBlockHeader, RecoveredBlock, SealedHeader, SignedTransaction, TransactionMeta,
};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert, RpcHeader};
use reth_storage_api::{BlockIdReader, BlockReader, ProviderHeader, ProviderReceipt, ProviderTx};
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{borrow::Cow, sync::Arc};

/// Result type of the fetched block receipts.
pub type BlockReceiptsResult<N, E> = Result<Option<Vec<RpcReceipt<N>>>, E>;
/// Result type of the fetched block and its receipts.
pub type BlockAndReceiptsResult<Eth> = Result<
    Option<(
        Arc<RecoveredBlock<<<Eth as RpcNodeCore>::Provider as BlockReader>::Block>>,
        Arc<Vec<ProviderReceipt<<Eth as RpcNodeCore>::Provider>>>,
    )>,
    <Eth as EthApiTypes>::Error,
>;

/// Block related functions for the [`EthApiServer`](crate::EthApiServer) trait in the
/// `eth_` namespace.
pub trait EthBlocks:
    LoadBlock<RpcConvert: RpcConvert<Primitives = Self::Primitives, Error = Self::Error>>
{
    /// Returns the block header for the given block id.
    fn rpc_block_header(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<RpcHeader<Self::NetworkTypes>>, Self::Error>> + Send
    where
        Self: FullEthApiTypes,
    {
        async move { Ok(self.rpc_block(block_id, false).await?.map(|block| block.header)) }
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
                |tx, tx_info| self.tx_resp_builder().fill(tx, tx_info),
                |header, size| self.tx_resp_builder().convert_header(header, size),
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
            if block_id.is_pending() {
                // Pending block can be fetched directly without need for caching
                return Ok(self
                    .provider()
                    .pending_block()
                    .map_err(Self::Error::from_eth_err)?
                    .map(|block| block.body().transaction_count()));
            }

            let block_hash = match self
                .provider()
                .block_hash_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
            {
                Some(block_hash) => block_hash,
                None => return Ok(None),
            };

            Ok(self
                .cache()
                .get_recovered_block(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?
                .map(|b| b.body().transaction_count()))
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
                    .zip(receipts.iter())
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

                        let input = ConvertReceiptInput {
                            receipt: Cow::Borrowed(receipt),
                            tx,
                            gas_used: receipt.cumulative_gas_used() - gas_used,
                            next_log_index,
                            meta,
                        };

                        gas_used = receipt.cumulative_gas_used();
                        next_log_index += receipt.logs().len();

                        input
                    })
                    .collect::<Vec<_>>();

                return self.tx_resp_builder().convert_receipts(inputs).map(Some)
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
                // First, try to get the pending block from the provider, in case we already
                // received the actual pending block from the CL.
                if let Some((block, receipts)) = self
                    .provider()
                    .pending_block_and_receipts()
                    .map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some((Arc::new(block), Arc::new(receipts))));
                }

                // If no pending block from provider, build the pending block locally.
                if let Some((block, receipts)) = self.local_pending_block().await? {
                    return Ok(Some((block, receipts)));
                }
            }

            if let Some(block_hash) =
                self.provider().block_hash_for_id(block_id).map_err(Self::Error::from_eth_err)?
            {
                if let Some((block, receipts)) = self
                    .cache()
                    .get_block_and_receipts(block_hash)
                    .await
                    .map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some((block, receipts)));
                }
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
            if let Some(block) = self.recovered_block(block_id).await? {
                Ok(block.body().ommers().map(|o| o.to_vec()))
            } else {
                Ok(None)
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
            let uncles = if block_id.is_pending() {
                // Pending block can be fetched directly without need for caching
                self.provider()
                    .pending_block()
                    .map_err(Self::Error::from_eth_err)?
                    .and_then(|block| block.body().ommers().map(|o| o.to_vec()))
            } else {
                self.recovered_block(block_id)
                    .await?
                    .map(|block| block.body().ommers().map(|o| o.to_vec()).unwrap_or_default())
            }
            .unwrap_or_default();

            uncles
                .into_iter()
                .nth(index.into())
                .map(|header| {
                    let block =
                        alloy_consensus::Block::<alloy_consensus::TxEnvelope, _>::uncle(header);
                    let size = block.length();
                    let header = self
                        .tx_resp_builder()
                        .convert_header(SealedHeader::new_unhashed(block.header), size)?;
                    Ok(Block {
                        uncles: vec![],
                        header,
                        transactions: BlockTransactions::Uncle,
                        withdrawals: None,
                        block_access_list: None,
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
                // Pending block can be fetched directly without need for caching
                if let Some(pending_block) =
                    self.provider().pending_block().map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some(Arc::new(pending_block)));
                }

                // If no pending block from provider, try to get local pending block
                return match self.local_pending_block().await? {
                    Some((block, _)) => Ok(Some(block)),
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
}
