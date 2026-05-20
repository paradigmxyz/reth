//! Loads chain metadata.

use alloy_primitives::{U256, U64};
use alloy_rpc_types_eth::{Stage, SyncInfo, SyncStatus};
use futures::Future;
use reth_chainspec::ChainInfo;
use reth_errors::{RethError, RethResult};
use reth_network_api::NetworkInfo;
use reth_prune_types::{PruneMode, PruneSegment};
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_types::{EthCapabilities, EthCapabilitiesHead, EthCapabilitiesResource};
use reth_storage_api::{
    BlockNumReader, PruneCheckpointReader, StageCheckpointReader, TransactionsProvider,
};

use crate::{helpers::EthSigner, EthApiTypes, RpcNodeCore};

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthApiSpec: RpcNodeCore + EthApiTypes {
    /// Returns the block node is started on.
    fn starting_block(&self) -> U256;

    /// Returns the current ethereum protocol version.
    fn protocol_version(&self) -> impl Future<Output = RethResult<U64>> + Send {
        async move {
            let status = self.network().network_status().await.map_err(RethError::other)?;
            Ok(U64::from(status.protocol_version))
        }
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        U64::from(self.network().chain_id())
    }

    /// Returns provider chain info
    fn chain_info(&self) -> RethResult<ChainInfo> {
        Ok(self.provider().chain_info()?)
    }

    /// Returns the maximum number of blocks into the past for generating state proofs.
    fn proof_window(&self) -> u64;

    /// Returns effective routing capabilities for this node.
    fn capabilities(&self) -> RethResult<EthCapabilities> {
        let chain_info = self.chain_info()?;
        let provider = self.provider();

        let state = effective_resource(
            provider,
            &[PruneSegment::AccountHistory, PruneSegment::StorageHistory],
        )?;
        let tx =
            effective_resource(provider, &[PruneSegment::TransactionLookup, PruneSegment::Bodies])?;
        let logs =
            effective_resource(provider, &[PruneSegment::Receipts, PruneSegment::ContractLogs])?;
        let receipts = effective_resource(
            provider,
            &[PruneSegment::Receipts, PruneSegment::TransactionLookup, PruneSegment::Bodies],
        )?;
        let blocks = effective_resource(provider, &[PruneSegment::Bodies])?;

        let proof_oldest = chain_info
            .best_number
            .saturating_sub(self.proof_window())
            .max(state.oldest_block.map(|number| number.to::<u64>()).unwrap_or_default());
        let state_proofs = EthCapabilitiesResource::window(proof_oldest, self.proof_window());

        Ok(EthCapabilities {
            head: EthCapabilitiesHead {
                number: U64::from(chain_info.best_number),
                hash: chain_info.best_hash,
            },
            state,
            tx,
            logs,
            receipts,
            blocks,
            state_proofs,
        })
    }

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool {
        self.network().is_syncing()
    }

    /// Returns the [`SyncStatus`] of the network
    fn sync_status(&self) -> RethResult<SyncStatus> {
        let status = if self.is_syncing() {
            let current_block = U256::from(
                self.provider().chain_info().map(|info| info.best_number).unwrap_or_default(),
            );

            let stages = self
                .provider()
                .get_all_checkpoints()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, checkpoint)| Stage { name, block: checkpoint.block_number })
                .collect();

            SyncStatus::Info(Box::new(SyncInfo {
                starting_block: self.starting_block(),
                current_block,
                highest_block: current_block,
                warp_chunks_amount: None,
                warp_chunks_processed: None,
                stages: Some(stages),
            }))
        } else {
            SyncStatus::None
        };
        Ok(status)
    }
}

/// Derives the effective block availability for a capability resource from the prune checkpoints of
/// all storage segments needed to serve it.
///
/// A resource is only available where every required segment is available, so the oldest available
/// block is the maximum pruned checkpoint across the segments. If any segment is configured with a
/// distance-based prune mode, expose the smallest retention window because that is the tightest
/// limit callers can rely on.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/755>
fn effective_resource(
    provider: &impl PruneCheckpointReader,
    segments: &[PruneSegment],
) -> RethResult<EthCapabilitiesResource> {
    let mut oldest_block = 0;
    let mut retention_blocks = None::<u64>;

    for segment in segments {
        let Some(checkpoint) = provider.get_prune_checkpoint(*segment)? else {
            continue;
        };

        if let Some(block_number) = checkpoint.block_number {
            oldest_block = oldest_block.max(block_number.saturating_add(1));
        }

        if let PruneMode::Distance(distance) = checkpoint.prune_mode {
            retention_blocks =
                Some(retention_blocks.map_or(distance, |current| current.min(distance)));
        }
    }

    Ok(if let Some(retention_blocks) = retention_blocks {
        EthCapabilitiesResource::window(oldest_block, retention_blocks)
    } else {
        EthCapabilitiesResource::available_from(oldest_block)
    })
}

/// A handle to [`EthSigner`]s with its generics set from [`TransactionsProvider`] and
/// [`reth_rpc_convert::RpcTypes`].
pub type SignersForRpc<Provider, Rpc> = parking_lot::RwLock<
    Vec<Box<dyn EthSigner<<Provider as TransactionsProvider>::Transaction, RpcTxReq<Rpc>>>>,
>;
