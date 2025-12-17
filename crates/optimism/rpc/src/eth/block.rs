//! Loads and formats OP block RPC response.

use crate::{eth::RpcNodeCore, OpEthApi, OpEthApiError};
use reth_node_api::BlockBody;
use reth_rpc_eth_api::{
    helpers::{EthBlocks, LoadBlock},
    FromEthApiError, FromEvmError, RpcConvert, RpcNodeCoreExt,
};
use reth_storage_api::{BlockIdReader, BlockReader};
impl<N, Rpc> EthBlocks for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
    fn block_transaction_count(
        &self,
        block_id: alloy_eips::BlockId,
    ) -> impl std::future::Future<Output = Result<Option<usize>, Self::Error>> + Send {
        async move {
            if block_id.is_pending() {
                if let Ok(Some(pending)) = self.pending_flashblock().await {
                    return Ok(Some(pending.block().body().transaction_count()));
                }

                if let Some(pending_block) =
                    self.provider().pending_block().map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some(pending_block.body().transaction_count()));
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

            Ok(self
                .cache()
                .get_recovered_block(block_hash)
                .await
                .map_err(Self::Error::from_eth_err)?
                .map(|b| b.body().transaction_count()))
        }
    }
}

impl<N, Rpc> LoadBlock for OpEthApi<N, Rpc>
where
    N: RpcNodeCore,
    OpEthApiError: FromEvmError<N::Evm>,
    Rpc: RpcConvert<Primitives = N::Primitives, Error = OpEthApiError>,
{
}
