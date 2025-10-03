use std::sync::Arc;

use crate::eth::ArbEthApi;
use reth_primitives_traits::RecoveredBlock;
use reth_rpc_eth_api::{
    helpers::{pending_block::PendingEnvBuilder, pending_block::LoadPendingBlock},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_rpc_eth_types::{builder::config::PendingBlockKind, EthApiError, PendingBlock};
use crate::error::ArbEthApiError;
use reth_storage_api::{BlockReader, BlockReaderIdExt, ProviderBlock, ProviderReceipt, ReceiptProvider};

impl<N, Rpc> LoadPendingBlock for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
    #[inline]
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<N::Primitives>>> {
        self.eth_api().pending_block()
    }

    #[inline]
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.eth_api().pending_env_builder()
    }
    #[inline]
    fn pending_block_kind(&self) -> PendingBlockKind {
        self.eth_api().pending_block_kind()
    }


    async fn local_pending_block(
        &self,
    ) -> Result<
        Option<(
            Arc<RecoveredBlock<ProviderBlock<Self::Provider>>>,
            Arc<Vec<ProviderReceipt<Self::Provider>>>,
        )>,
        Self::Error,
    > {
        let latest = self
            .provider()
            .latest_header()?
            .ok_or_else(|| ArbEthApiError::Eth(EthApiError::HeaderNotFound(alloy_eips::BlockNumberOrTag::Latest.into())))?;
        let block_id = latest.hash().into();
        let block = self
            .provider()
            .recovered_block(block_id, Default::default())?
            .ok_or_else(|| ArbEthApiError::Eth(EthApiError::HeaderNotFound(block_id.into())))?;

        let receipts = self
            .provider()
            .receipts_by_block(block_id)?
            .ok_or_else(|| ArbEthApiError::Eth(EthApiError::ReceiptsNotFound(block_id.into())))?;

        Ok(Some((Arc::new(block), Arc::new(receipts))))
    }
}
