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
use reth_stages_types::StageId;
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
            let current_block =
                self.provider().chain_info().map(|info| info.best_number).unwrap_or_default();

            let stages: Vec<Stage> = self
                .provider()
                .get_all_checkpoints()
                .unwrap_or_default()
                .into_iter()
                .map(|(name, checkpoint)| Stage { name, block: checkpoint.block_number })
                .collect();
            let highest_block = estimate_highest_block(
                current_block,
                stages.iter().map(|stage| (stage.name.as_str(), stage.block)),
            );

            SyncStatus::Info(Box::new(SyncInfo {
                starting_block: self.starting_block(),
                current_block: U256::from(current_block),
                highest_block: U256::from(highest_block),
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

/// Estimates the highest known block from the validated headers stage checkpoint.
///
/// The checkpoint can briefly lag the canonical head while a header gap is being committed, so the
/// returned height never falls below `current_block`.
pub fn estimate_highest_block<S>(
    current_block: u64,
    stages: impl IntoIterator<Item = (S, u64)>,
) -> u64
where
    S: AsRef<str>,
{
    stages
        .into_iter()
        .find_map(|(name, block)| (name.as_ref() == StageId::Headers.as_str()).then_some(block))
        .map_or(current_block, |block| block.max(current_block))
}

/// Derives the effective block availability for a capability resource from the prune checkpoints of
/// all storage segments needed to serve it.
///
/// A resource is only available where every required segment is available, so the oldest available
/// block is the maximum pruned checkpoint across the segments. If any segment is configured with a
/// distance-based prune mode, or with full pruning that still keeps the segment minimum, expose the
/// smallest retention window because that is the tightest limit callers can rely on. If full
/// pruning leaves no reliable window for any required segment, the resource is disabled.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/755>
fn effective_resource(
    provider: &impl PruneCheckpointReader,
    segments: &[PruneSegment],
) -> RethResult<EthCapabilitiesResource> {
    let mut oldest_block = 0;
    let mut retention_blocks = None::<u64>;
    let mut disabled = false;

    for segment in segments {
        let Some(checkpoint) = provider.get_prune_checkpoint(*segment)? else {
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

    Ok(if disabled {
        EthCapabilitiesResource::disabled()
    } else if let Some(retention_blocks) = retention_blocks {
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_prune_types::PruneCheckpoint;
    use reth_storage_api::errors::provider::ProviderResult;
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestPruneCheckpointReader {
        checkpoints: HashMap<PruneSegment, PruneCheckpoint>,
    }

    impl TestPruneCheckpointReader {
        fn with_checkpoint(segment: PruneSegment, checkpoint: PruneCheckpoint) -> Self {
            Self { checkpoints: HashMap::from([(segment, checkpoint)]) }
        }
    }

    impl PruneCheckpointReader for TestPruneCheckpointReader {
        fn get_prune_checkpoint(
            &self,
            segment: PruneSegment,
        ) -> ProviderResult<Option<PruneCheckpoint>> {
            Ok(self.checkpoints.get(&segment).copied())
        }

        fn get_prune_checkpoints(&self) -> ProviderResult<Vec<(PruneSegment, PruneCheckpoint)>> {
            Ok(self
                .checkpoints
                .iter()
                .map(|(segment, checkpoint)| (*segment, *checkpoint))
                .collect())
        }
    }

    #[test]
    fn highest_block_uses_headers_stage_checkpoint() {
        let stages = [(StageId::Bodies.as_str(), 20), (StageId::Headers.as_str(), 30)];

        assert_eq!(estimate_highest_block(10, stages), 30);
    }

    #[test]
    fn highest_block_does_not_fall_behind_current_block() {
        let stages = [(StageId::Headers.as_str(), 10)];

        assert_eq!(estimate_highest_block(20, stages), 20);
    }

    #[test]
    fn highest_block_falls_back_to_current_block_without_headers_checkpoint() {
        let stages = [(StageId::Bodies.as_str(), 30)];

        assert_eq!(estimate_highest_block(20, stages), 20);
    }

    #[test]
    fn full_prune_segment_without_retention_disables_resource() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::TransactionLookup,
            PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full,
            },
        );

        let resource = effective_resource(&provider, &[PruneSegment::TransactionLookup]).unwrap();

        assert_eq!(resource, EthCapabilitiesResource::disabled());
    }

    #[test]
    fn full_prune_segment_with_minimum_retention_uses_window() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Bodies,
            PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full,
            },
        );

        let resource = effective_resource(&provider, &[PruneSegment::Bodies]).unwrap();

        assert_eq!(
            resource,
            EthCapabilitiesResource::window(11, PruneSegment::Bodies.min_blocks())
        );
    }
}
