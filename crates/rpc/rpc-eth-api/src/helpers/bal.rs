//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eip7928::{bal::DecodedBal, BlockAccessList};
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::BlockId;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_errors::RethError;
use reth_evm::{database::StateProviderDatabase, BlockExecutor, ConfigureEvm};
use reth_rpc_eth_types::{error::FromEthApiError, EthApiError};
use reth_storage_api::{StateProvider, StateProviderFactory};
use std::sync::Arc;

use crate::{
    helpers::{LoadBlock, Trace},
    RpcNodeCore,
};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + LoadBlock {
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

            if !self.provider().chain_spec().is_amsterdam_active_at_timestamp(block.timestamp()) {
                return Err(Self::Error::from_eth_err(
                    EthApiError::BlockAccessListNotAvailablePreAmsterdam,
                ));
            }

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
                let state = eth_api
                    .provider()
                    .state_by_block_id(block.parent_hash().into())
                    .map_err(Self::Error::from_eth_err)?;

                let db = StateProviderDatabase::new(state.into_evm_state_provider());
                let mut executor = RpcNodeCore::evm_config(&eth_api)
                    .executor_for_block(db, block.sealed_block())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?;
                executor.enable_block_access_list_builder();
                executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;
                for block_tx in block.transactions_recovered() {
                    executor.execute_transaction(block_tx).map_err(Self::Error::from_eth_err)?;
                }
                let (_, bal) = executor
                    .finish_with_prepared_block_access_list()
                    .map_err(Self::Error::from_eth_err)?;
                let Some(bal) = bal else { return Ok(None) };
                if !eth_api.eth_api_settings().cache_computed_bals {
                    return Ok(Some(BlockAccessList::from(bal)))
                }

                let alloy_bal = BlockAccessList::from(&bal);
                let raw = alloy_rlp::encode(&alloy_bal).into();
                eth_api.cache().insert_bal(block.hash(), DecodedBal::new(Arc::new(bal), raw));
                Ok(Some(alloy_bal))
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

            if !self.provider().chain_spec().is_amsterdam_active_at_timestamp(block.timestamp()) {
                return Err(Self::Error::from_eth_err(
                    EthApiError::BlockAccessListNotAvailablePreAmsterdam,
                ));
            }

            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(cached_bal.as_raw().clone()))
            }

            Ok(self.get_block_access_list(block_id).await?.map(|bal| alloy_rlp::encode(bal).into()))
        }
    }
}
