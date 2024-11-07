//! Loads and formats OP block RPC response.

use alloy_rpc_types::BlockId;
use op_alloy_network::Network;
use op_alloy_rpc_types::OpTransactionReceipt;
use reth_chainspec::ChainSpecProvider;
use reth_optimism_chainspec::OpChainSpec;
use reth_primitives::TransactionMeta;
use reth_provider::HeaderProvider;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock, LoadPendingBlock, LoadReceipt, SpawnBlocking},
    RpcNodeCore, RpcReceipt,
};

use crate::{OpEthApi, OpEthApiError, OpReceiptBuilder};

impl<N> EthBlocks for OpEthApi<N>
where
    Self: LoadBlock<
        Error = OpEthApiError,
        NetworkTypes: Network<ReceiptResponse = OpTransactionReceipt>,
    >,
    N: RpcNodeCore<Provider: ChainSpecProvider<ChainSpec = OpChainSpec> + HeaderProvider>,
{
    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<RpcReceipt<Self::NetworkTypes>>>, Self::Error>
    where
        Self: LoadReceipt,
    {
        if let Some((block, receipts)) = self.load_block_and_receipts(block_id).await? {
            let block_number = block.number;
            let base_fee = block.base_fee_per_gas;
            let block_hash = block.hash();
            let excess_blob_gas = block.excess_blob_gas;
            let timestamp = block.timestamp;
            let block = block.unseal();

            let l1_block_info =
                reth_optimism_evm::extract_l1_info(&block.body).map_err(OpEthApiError::from)?;

            return block
                .body
                .transactions
                .into_iter()
                .zip(receipts.iter())
                .enumerate()
                .map(|(idx, (ref tx, receipt))| -> Result<_, _> {
                    let meta = TransactionMeta {
                        tx_hash: tx.hash,
                        index: idx as u64,
                        block_hash,
                        block_number,
                        base_fee,
                        excess_blob_gas,
                        timestamp,
                    };

                    Ok(OpReceiptBuilder::new(
                        &self.inner.provider().chain_spec(),
                        tx,
                        meta,
                        receipt,
                        &receipts,
                        l1_block_info.clone(),
                    )?
                    .build())
                })
                .collect::<Result<Vec<_>, Self::Error>>()
                .map(Some)
        }

        Ok(None)
    }
}

impl<N> LoadBlock for OpEthApi<N>
where
    Self: LoadPendingBlock + SpawnBlocking,
    N: RpcNodeCore,
{
}
