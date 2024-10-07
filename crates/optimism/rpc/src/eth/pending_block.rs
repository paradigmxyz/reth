//! Loads OP pending block for a RPC response.

use alloy_primitives::{BlockNumber, B256};
use reth_chainspec::EthereumHardforks;
use reth_evm::ConfigureEvm;
use reth_node_api::{FullNodeComponents, NodeTypes};
use reth_optimism_consensus::calculate_receipt_root_no_memo_optimism;
use reth_primitives::{
    revm_primitives::BlockEnv, BlockNumberOrTag, Header, Receipt, SealedBlockWithSenders,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome,
    ReceiptProvider, StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_transaction_pool::TransactionPool;

use crate::OpEthApi;

impl<N> LoadPendingBlock for OpEthApi<N>
where
    Self: SpawnBlocking,
    N: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt
           + EvmEnvProvider
           + ChainSpecProvider<ChainSpec: EthereumHardforks>
           + StateProviderFactory {
        self.inner.provider()
    }

    #[inline]
    fn pool(&self) -> impl TransactionPool {
        self.inner.pool()
    }

    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock>> {
        self.inner.pending_block()
    }

    #[inline]
    fn evm_config(&self) -> &impl ConfigureEvm<Header = Header> {
        self.inner.evm_config()
    }

    /// Returns the locally built pending block
    async fn local_pending_block(
        &self,
    ) -> Result<Option<(SealedBlockWithSenders, Vec<Receipt>)>, Self::Error> {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let block_id = latest.hash().into();
        let block = self
            .provider()
            .block_with_senders(block_id, Default::default())
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id.into()))?
            .seal(latest.hash());

        let receipts = self
            .provider()
            .receipts_by_block(block_id)
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::ReceiptsNotFound(block_id.into()))?;

        Ok(Some((block, receipts)))
    }

    fn receipts_root(
        &self,
        block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome
            .generic_receipts_root_slow(block_number, |receipts| {
                calculate_receipt_root_no_memo_optimism(
                    receipts,
                    self.provider().chain_spec().as_ref(),
                    block_env.timestamp.to::<u64>(),
                )
            })
            .expect("Block is present")
    }
}
