//! Helpers for `eth_blockAccessList` RPC method.
use alloy_eip7928::{bal::DecodedBal, BlockAccessList};
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_rpc_eth_types::{bal::build_revm_bal_for_block, error::FromEthApiError, EthApiError};

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCore, RpcNodeCoreExt,
};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock + RpcNodeCoreExt {
    /// Retrieves the block access list for a block identified by its hash.
    fn get_block_access_list(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<BlockAccessList>, Self::Error>> + Send {
        async move {
            if block_id.is_pending() {
                return Ok(None)
            }

            let Some(block) = self.recovered_block(block_id).await? else {
                return Ok(None);
            };

            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                let (bal, _) = DecodedBal::from_rlp_bytes(cached_bal.as_raw().clone())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?
                    .split();
                return Ok(Some(Vec::from(bal)))
            }

            let permit = self
                .acquire_owned_blocking_io()
                .await
                .map_err(|_| EthApiError::InternalEthError)?;

            self.spawn_blocking_io(move |eth_api| {
                let _permit = permit;
                let bal = build_revm_bal_for_block(
                    eth_api.provider(),
                    RpcNodeCore::evm_config(&eth_api),
                    &block,
                )
                .map_err(Self::Error::from_eth_err)?;
                let (response, _) = DecodedBal::from_rlp_bytes(bal.as_raw().clone())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?
                    .split();
                eth_api.cache().insert_bal(block.hash(), bal);
                Ok(Some(Vec::from(response)))
            })
            .await
        }
    }

    /// Retrieves the raw RLP-encoded block access list for a block.
    fn get_raw_block_access_list(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send {
        async move {
            let block = self
                .recovered_block(block_id)
                .await?
                .ok_or_else(|| EthApiError::HeaderNotFound(block_id))?;

            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(cached_bal.as_raw().clone()))
            }

            Ok(self.get_block_access_list(block_id).await?.map(|bal| alloy_rlp::encode(bal).into()))
        }
    }
}
