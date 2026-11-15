//! Loads chain metadata.

use alloy_primitives::{U256, U64};
use alloy_rpc_types_eth::{Stage, SyncInfo, SyncStatus};
use futures::Future;
use reth_chainspec::ChainInfo;
use reth_errors::{RethError, RethResult};
use reth_network_api::NetworkInfo;
use reth_prune_types::{PruneCheckpoint, PruneMode, PruneSegment};
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_types::{EthCapabilities, EthCapabilitiesHead, EthCapabilitiesResource};
use reth_storage_api::{
    BlockNumReader, PruneCheckpointReader, StageCheckpointReader, TransactionsProvider,
};

use crate::{
    helpers::{EthSigner, EthState},
    EthApiTypes, RpcNodeCore,
};

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

    /// Returns effective routing capabilities for this node.
    ///
    /// The response follows the `eth_capabilities` execution API proposal:
    /// <https://github.com/ethereum/execution-apis/pull/755>.
    fn capabilities(&self) -> RethResult<EthCapabilities>
    where
        Self: EthState,
    {
        let chain_info = self.chain_info()?;
        let checkpoints = self.provider().get_prune_checkpoints_for_availability()?;

        let state = effective_resource(
            &checkpoints,
            &[PruneSegment::AccountHistory, PruneSegment::StorageHistory],
        );
        let tx = effective_resource(
            &checkpoints,
            &[PruneSegment::TransactionLookup, PruneSegment::Bodies],
        );
        let logs =
            effective_resource(&checkpoints, &[PruneSegment::Receipts, PruneSegment::ContractLogs]);
        let receipts = effective_resource(
            &checkpoints,
            &[PruneSegment::Receipts, PruneSegment::TransactionLookup, PruneSegment::Bodies],
        );
        let blocks = effective_resource(&checkpoints, &[PruneSegment::Bodies]);

        let proof_window = self.max_proof_window();
        let proof_oldest = chain_info
            .best_number
            .saturating_sub(proof_window)
            .max(state.oldest_block.map(|number| number.to::<u64>()).unwrap_or_default());
        let state_proofs = EthCapabilitiesResource::window(proof_oldest, proof_window);

        Ok(EthCapabilities {
            head: EthCapabilitiesHead {
                number: chain_info.best_number,
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

/// Derives effective block availability from checkpoints adjusted for retained data by
/// [`PruneCheckpointReader::get_prune_checkpoints_for_availability`].
///
/// A resource is only available where every required segment is available, so the oldest available
/// block is the maximum pruned checkpoint across the segments. If any segment is configured with a
/// distance-based prune mode, or with full pruning that still keeps the segment minimum, expose the
/// smallest retention window because that is the tightest limit callers can rely on. If full
/// pruning leaves no reliable window for any required segment, the resource is disabled.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/755>
fn effective_resource(
    checkpoints: &[(PruneSegment, PruneCheckpoint)],
    segments: &[PruneSegment],
) -> EthCapabilitiesResource {
    let mut oldest_block = 0;
    let mut retention_blocks = None::<u64>;
    let mut disabled = false;

    for segment in segments {
        let Some((_, checkpoint)) = checkpoints.iter().find(|(key, _)| key == segment) else {
            continue;
        };

        if let Some(block_number) = checkpoint.block_number {
            oldest_block = oldest_block.max(block_number.saturating_add(1));
        }

        match checkpoint.prune_mode {
            PruneMode::Distance(distance) => {
                retention_blocks =
                    Some(retention_blocks.map_or(distance, |current| current.min(distance)));
            }
            PruneMode::Full => {
                let min_blocks = segment.min_blocks();
                if min_blocks == 0 {
                    disabled = true;
                } else {
                    retention_blocks = Some(
                        retention_blocks.map_or(min_blocks, |current| current.min(min_blocks)),
                    );
                }
            }
            PruneMode::Before(_) => {}
        }
    }

    if disabled {
        EthCapabilitiesResource::disabled()
    } else if let Some(retention_blocks) = retention_blocks {
        EthCapabilitiesResource::window(oldest_block, retention_blocks)
    } else {
        EthCapabilitiesResource::available_from(oldest_block)
    }
}

/// A handle to [`EthSigner`]s with its generics set from [`TransactionsProvider`] and
/// [`reth_rpc_convert::RpcTypes`].
pub type SignersForRpc<Provider, Rpc> = parking_lot::RwLock<
    Vec<Box<dyn EthSigner<<Provider as TransactionsProvider>::Transaction, RpcTxReq<Rpc>>>>,
>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_prune_segment_without_retention_disables_resource() {
        let checkpoints = [(
            PruneSegment::TransactionLookup,
            PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full,
            },
        )];

        let resource = effective_resource(&checkpoints, &[PruneSegment::TransactionLookup]);

        assert_eq!(resource, EthCapabilitiesResource::disabled());
    }

    #[test]
    fn full_prune_segment_with_minimum_retention_uses_window() {
        let checkpoints = [(
            PruneSegment::Bodies,
            PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full,
            },
        )];

        let resource = effective_resource(&checkpoints, &[PruneSegment::Bodies]);

        assert_eq!(
            resource,
            EthCapabilitiesResource::window(11, PruneSegment::Bodies.min_blocks())
        );
    }

    #[test]
    fn retained_receipts_preserve_resource_limits() {
        let checkpoints = [
            (
                PruneSegment::Receipts,
                PruneCheckpoint {
                    block_number: None,
                    tx_number: None,
                    prune_mode: PruneMode::Distance(100),
                },
            ),
            (
                PruneSegment::ContractLogs,
                PruneCheckpoint {
                    prune_mode: PruneMode::Distance(50),
                    ..PruneCheckpoint::pruned_through(19)
                },
            ),
            (PruneSegment::TransactionLookup, PruneCheckpoint::pruned_through(39)),
            (PruneSegment::Bodies, PruneCheckpoint::pruned_through(29)),
        ];
        for (segments, expected) in [
            (vec![PruneSegment::Receipts], EthCapabilitiesResource::window(0, 100)),
            (
                vec![PruneSegment::Receipts, PruneSegment::ContractLogs],
                EthCapabilitiesResource::window(20, 50),
            ),
            (
                vec![PruneSegment::Receipts, PruneSegment::TransactionLookup, PruneSegment::Bodies],
                EthCapabilitiesResource::window(40, 100),
            ),
            (vec![PruneSegment::Bodies], EthCapabilitiesResource::available_from(30)),
            (vec![PruneSegment::AccountHistory], EthCapabilitiesResource::available_from(0)),
        ] {
            assert_eq!(effective_resource(&checkpoints, &segments), expected, "{segments:?}");
        }
    }
}
