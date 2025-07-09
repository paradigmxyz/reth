//! Contains RPC handler implementations specific to blocks.

use alloy_consensus::{transaction::TransactionMeta, BlockHeader};
use alloy_rpc_types_eth::{BlockId, TransactionReceipt};
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_evm::ConfigureEvm;
use reth_primitives_traits::NodePrimitives;
use reth_rpc_convert::RpcConvert;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, LoadReceipt, SpawnBlocking},
    types::RpcTypes,
    RpcNodeCore, RpcNodeCoreExt, RpcReceipt,
};
use reth_rpc_eth_types::{EthApiError, EthReceiptBuilder};
use reth_storage_api::{BlockReader, ProviderTx};
use reth_transaction_pool::{PoolTransaction, TransactionPool};

use crate::EthApi;

impl<Provider, Pool, Network, EvmConfig> EthBlocks for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadBlock<
        Error = EthApiError,
        NetworkTypes: RpcTypes<Receipt = TransactionReceipt>,
        RpcConvert: RpcConvert<Network = Self::NetworkTypes>,
        Provider: BlockReader<
            Transaction = reth_ethereum_primitives::TransactionSigned,
            Receipt = reth_ethereum_primitives::Receipt,
        >,
    >,
    Provider: BlockReader + ChainSpecProvider,
{
    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<RpcReceipt<Self::NetworkTypes>>>, Self::Error>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number();
            let base_fee = block.base_fee_per_gas();
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas();
            let timestamp = block.timestamp();
            let blob_params = self.provider().chain_spec().blob_params_at_timestamp(timestamp);

            return block
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
                    Ok(EthReceiptBuilder::new(tx, meta, receipt, &receipts, blob_params).build())
                })
                .collect::<Result<Vec<_>, Self::Error>>()
                .map(Some)
        }

        Ok(None)
    }
}

impl<Provider, Pool, Network, EvmConfig> LoadBlock for EthApi<Provider, Pool, Network, EvmConfig>
where
    Self: LoadPendingBlock
        + SpawnBlocking
        + RpcNodeCoreExt<
            Pool: TransactionPool<
                Transaction: PoolTransaction<Consensus = ProviderTx<Self::Provider>>,
            >,
            Primitives: NodePrimitives<SignedTx = ProviderTx<Self::Provider>>,
            Evm = EvmConfig,
        >,
    Provider: BlockReader,
    EvmConfig: ConfigureEvm<Primitives = <Self as RpcNodeCore>::Primitives>,
{
}
