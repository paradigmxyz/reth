//! Loads OP pending block for a RPC response.   

use crate::OpEthApi;
use jsonrpsee::tracing::debug;
use reth_evm::ConfigureEvm;
use reth_node_api::FullNodeComponents;
use reth_primitives::{
    revm_primitives::BlockEnv, BlockHashOrNumber, BlockNumber, SealedBlockWithSenders, B256,
};
use reth_provider::{
    BlockReader, BlockReaderIdExt, ChainSpecProvider, EvmEnvProvider, ExecutionOutcome,
    StateProviderFactory,
};
use reth_rpc_eth_api::{
    helpers::{LoadPendingBlock, SpawnBlocking},
    FromEthApiError,
};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_transaction_pool::TransactionPool;
use std::time::{Duration, Instant};

impl<N> LoadPendingBlock for OpEthApi<N>
where
    Self: SpawnBlocking,
    N: FullNodeComponents,
{
    #[inline]
    fn provider(
        &self,
    ) -> impl BlockReaderIdExt + EvmEnvProvider + ChainSpecProvider + StateProviderFactory {
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
    fn evm_config(&self) -> &impl ConfigureEvm {
        self.inner.evm_config()
    }

    /// Returns the locally built pending block
    async fn local_pending_block(&self) -> Result<Option<SealedBlockWithSenders>, Self::Error> {
        let pending = self.pending_block_env_and_cfg()?;
        if pending.origin.is_actual_pending() {
            return Ok(pending.origin.into_actual_pending())
        }

        let mut lock = self.pending_block().lock().await;

        let now = Instant::now();

        // check if the block is still good
        if let Some(pending_block) = lock.as_ref() {
            // this is guaranteed to be the `latest` header
            if pending.block_env.number.to::<u64>() == pending_block.block.number &&
                pending.origin.header().hash() == pending_block.block.parent_hash &&
                now <= pending_block.expires_at
            {
                return Ok(Some(pending_block.block.clone()))
            }
        }

        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or_else(|| EthApiError::UnknownBlockNumber)?;
        let (_, block_hash) = latest.split();
        let last_block = self
            .provider()
            .sealed_block_with_senders(BlockHashOrNumber::from(block_hash), Default::default())
            .map_err(Self::Error::from_eth_err)?;

        let pending_block = match last_block {
            Some(pending_block) => pending_block,
            None => {
                debug!(target: "rpc", "Failed to build pending block get latest block");
                return Ok(None)
            }
        };

        let now = Instant::now();
        *lock = Some(PendingBlock::new(pending_block.clone(), now + Duration::from_secs(1)));

        Ok(Some(pending_block))
    }

    fn receipts_root(
        &self,
        _block_env: &BlockEnv,
        execution_outcome: &ExecutionOutcome,
        block_number: BlockNumber,
    ) -> B256 {
        execution_outcome
            .optimism_receipts_root_slow(
                block_number,
                self.provider().chain_spec().as_ref(),
                _block_env.timestamp.to::<u64>(),
            )
            .expect("Block is present")
    }
}
