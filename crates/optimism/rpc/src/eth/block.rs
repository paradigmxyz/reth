//! Loads and formats OP block RPC response.

use reth_chainspec::ChainSpec;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_primitives::TransactionMeta;
use reth_provider::{BlockReaderIdExt, HeaderProvider};
use reth_rpc_eth_api::{
    helpers::{
        EthApiSpec, EthBlocks, LoadBlock, LoadPendingBlock, LoadReceipt, LoadTransaction,
        SpawnBlocking,
    },
    FromEthApiError,
};
use reth_rpc_eth_types::{EthStateCache, ReceiptBuilder};
use reth_rpc_types::{AnyTransactionReceipt, BlockId};

use crate::{OpEthApi, OpEthApiError};

impl<N> EthBlocks for OpEthApi<N>
where
    Self: EthApiSpec + LoadBlock<Error = OpEthApiError> + LoadTransaction,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec = ChainSpec>>,
{
    #[inline]
    fn provider(&self) -> impl HeaderProvider {
        self.inner.provider()
    }

    async fn block_receipts(
        &self,
        block_id: BlockId,
    ) -> Result<Option<Vec<AnyTransactionReceipt>>, Self::Error>
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
                reth_evm_optimism::extract_l1_info(&block).map_err(OpEthApiError::from)?;

            let receipts = block
                .body
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

                    let op_tx_meta =
                        self.build_op_receipt_meta(tx, l1_block_info.clone(), receipt)?;

                    Ok(ReceiptBuilder::new(tx, meta, receipt, &receipts)
                        .map_err(Self::Error::from_eth_err)?
                        .add_other_fields(op_tx_meta.into())
                        .build())
                })
                .collect::<Result<Vec<_>, Self::Error>>();
            return receipts.map(Some)
        }

        Ok(None)
    }
}

impl<N> LoadBlock for OpEthApi<N>
where
    Self: LoadPendingBlock + SpawnBlocking,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(&self) -> impl BlockReaderIdExt {
        self.inner.provider()
    }

    #[inline]
    fn cache(&self) -> &EthStateCache {
        self.inner.cache()
    }
}
