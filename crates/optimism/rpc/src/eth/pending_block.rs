//! Loads OP pending block for a RPC response.

use crate::OpEthApi;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use reth_evm::ConfigureEvm;
use reth_optimism_evm::OpNextBlockEnvAttributes;
use reth_primitives_traits::{BlockTy, HeaderTy, ReceiptTy, RecoveredBlock, SealedHeader};
use reth_rpc_eth_api::{helpers::LoadPendingBlock, FromEthApiError, FullEthApiTypes, RpcNodeCore};
use reth_rpc_eth_types::{EthApiError, PendingBlock};
use reth_storage_api::{BlockReader, BlockReaderIdExt, ReceiptProvider};

use super::OpNodeCore;

impl<N> LoadPendingBlock for OpEthApi<N>
where
    N: OpNodeCore<Evm: ConfigureEvm<NextBlockEnvCtx = OpNextBlockEnvAttributes>>,
    Self: FullEthApiTypes<Primitives = N::Primitives, Evm = N::Evm>,
{
    #[inline]
    fn pending_block(
        &self,
    ) -> &tokio::sync::Mutex<
        Option<PendingBlock<BlockTy<Self::Primitives>, ReceiptTy<Self::Primitives>>>,
    > {
        self.inner.eth_api.pending_block()
    }

    fn next_env_attributes(
        &self,
        parent: &SealedHeader<HeaderTy<Self::Primitives>>,
    ) -> Result<<Self::Evm as reth_evm::ConfigureEvm>::NextBlockEnvCtx, Self::Error> {
        Ok(OpNextBlockEnvAttributes {
            timestamp: parent.timestamp().saturating_add(12),
            suggested_fee_recipient: parent.beneficiary(),
            prev_randao: B256::random(),
            gas_limit: parent.gas_limit(),
            parent_beacon_block_root: parent.parent_beacon_block_root(),
            extra_data: parent.extra_data().clone(),
        })
    }

    /// Returns the locally built pending block
    async fn local_pending_block(
        &self,
    ) -> Result<
        Option<(RecoveredBlock<BlockTy<Self::Primitives>>, Vec<ReceiptTy<Self::Primitives>>)>,
        Self::Error,
    > {
        // See: <https://github.com/ethereum-optimism/op-geth/blob/f2e69450c6eec9c35d56af91389a1c47737206ca/miner/worker.go#L367-L375>
        let latest = self
            .provider()
            .latest_header()
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(BlockNumberOrTag::Latest.into()))?;
        let block_id = latest.hash().into();
        let block = self
            .provider()
            .recovered_block(block_id, Default::default())
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::HeaderNotFound(block_id.into()))?;

        let receipts = self
            .provider()
            .receipts_by_block(block_id)
            .map_err(Self::Error::from_eth_err)?
            .ok_or(EthApiError::ReceiptsNotFound(block_id.into()))?;

        Ok(Some((block, receipts)))
    }
}
