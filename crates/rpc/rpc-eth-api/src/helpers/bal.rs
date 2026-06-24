//! Helpers for `eth_blockAccessList` RPC method.
#[cfg(any())]
use alloy_eip7928::bal::DecodedBal;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::BlockId;
#[cfg(any())]
use reth_errors::RethError;
use reth_rpc_eth_types::{error::FromEthApiError, EthApiError};

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCoreExt,
};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock + RpcNodeCoreExt {
    /// Retrieves the block access list for a block identified by its hash.
    fn get_block_access_list(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<BlockAccessList>, Self::Error>> + Send {
        async move {
            let block = self
                .recovered_block(block_id)
                .await?
                .ok_or_else(|| EthApiError::HeaderNotFound(block_id))?;
            let _ = block;

            #[cfg(any())]
            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                let (bal, _) = DecodedBal::from_rlp_bytes(cached_bal.as_raw().clone())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?
                    .split();
                return Ok(Some(Vec::from(bal)))
            }

            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "block access list construction is unsupported by the active EVM execution path",
            )))

            // The previous live construction path is intentionally kept out of the
            // compiled execution path. Restore this block when BAL execution is wired through the
            // active EVM:
            //
            // self.spawn_blocking_io(move |eth_api| {
            //     let state = eth_api
            //         .provider()
            //         .state_by_block_id(block.parent_hash().into())
            //         .map_err(Self::Error::from_eth_err)?;
            //
            //     let mut db = State::builder()
            //         .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(state)))
            //         .with_bal_builder()
            //         .build();
            //
            //     let block_txs = block.transactions_recovered();
            //     let mut executor = RpcNodeCore::evm_config(&eth_api)
            //         .executor_for_block(&mut db, block.sealed_block())
            //         .map_err(RethError::other)
            //         .map_err(Self::Error::from_eth_err)?;
            //
            //     executor
            //         .apply_pre_execution_changes()
            //         .map_err(reth_errors::BlockExecutionError::other)
            //         .map_err(Self::Error::from_eth_err)?;
            //     executor.evm_mut().db_mut().bump_bal_index();
            //
            //     for block_tx in block_txs {
            //         executor
            //             .execute_transaction(block_tx)
            //             .map_err(reth_errors::BlockExecutionError::other)
            //             .map_err(Self::Error::from_eth_err)?;
            //         executor.evm_mut().db_mut().bump_bal_index();
            //     }
            //
            //     executor
            //         .apply_post_execution_changes()
            //         .map_err(reth_errors::BlockExecutionError::other)
            //         .map_err(|err| EthApiError::Internal(err.into()))?;
            //
            //     let bal = db.take_built_alloy_bal();
            //     Ok(bal)
            // })
            // .await
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
            let _ = block;

            #[cfg(any())]
            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(cached_bal.as_raw().clone()))
            }

            Err(Self::Error::from_eth_err(EthApiError::Unsupported(
                "raw block access lists are unsupported by the active EVM execution path",
            )))

            // The previous cached/raw BAL path is intentionally kept out of the
            // compiled execution path. Restore this block when BAL execution is wired through the
            // active EVM:
            //
            // Ok(self
            //     .get_block_access_list(block_id)
            //     .await?
            //     .map(|bal| alloy_rlp::encode(bal).into()))
        }
    }
}
