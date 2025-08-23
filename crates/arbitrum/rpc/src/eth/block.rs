use crate::eth::ArbEthApi;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_storage_api::{BlockIdReader, BlockReader, HeaderProvider, ProviderHeader};
use crate::error::ArbEthApiError;
use reth_primitives_traits::RecoveredBlock;
use reth_rpc_eth_types::EthApiError;
use std::sync::Arc;
use alloy_eips::BlockId;

impl<N, Rpc> EthBlocks for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}

impl<N, Rpc> LoadBlock for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore<Provider: HeaderProvider + BlockReader + BlockIdReader>,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
    fn recovered_block(
        &self,
        block_id: BlockId,
    ) -> impl core::future::Future<
        Output = Result<
            Option<Arc<RecoveredBlock<<Self::Provider as BlockReader>::Block>>>,
            Self::Error,
        >,
    > + Send {
        async move {
            if block_id.is_pending() {
                if let Some(pending_block) =
                    self.provider().pending_block().map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some(Arc::new(pending_block)));
                }
                if let Some((block, _)) = self.local_pending_block().await? {
                    return Ok(Some(block));
                }
                return Ok(None);
            }

            let block_hash = match self
                .provider()
                .block_hash_for_id(block_id)
                .map_err(Self::Error::from_eth_err)?
            {
                Some(block_hash) => block_hash,
                None => return Ok(None),
            };

            if let Some(block) = self
                .cache()
                .get_recovered_block(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(block));
            }

            let from_provider = self
                .provider()
                .recovered_block(block_hash.into(), Default::default())
                .map_err(Self::Error::from_eth_err)?;
            Ok(from_provider.map(Arc::new))
        }
    }
}

use crate::eth::ArbEthApi;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FromEvmError, RpcConvert, RpcNodeCore,
};
use reth_storage_api::HeaderProvider;
use crate::error::ArbEthApiError;

impl<N, Rpc> EthBlocks for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}

impl<N, Rpc> LoadBlock for ArbEthApi<N, Rpc>
where
    N: RpcNodeCore<Provider: HeaderProvider>,
    ArbEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = ArbEthApiError>,
{
}
