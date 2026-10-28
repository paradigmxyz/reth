//! Loads chain metadata.

use alloy_primitives::{BlockNumber, TxNumber, U256, U64};
use alloy_rpc_types_eth::{Stage, SyncInfo, SyncStatus};
use futures::Future;
use reth_chainspec::ChainInfo;
use reth_errors::{RethError, RethResult};
use reth_network_api::NetworkInfo;
use reth_prune_types::{PruneMode, PruneSegment};
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_types::{EthCapabilities, EthCapabilitiesHead, EthCapabilitiesResource};
use reth_storage_api::{
    errors::provider::ProviderResult, BlockNumReader, PruneCheckpointReader, ReceiptProvider,
    StageCheckpointReader, TransactionsProvider,
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
        let static_file_receipts_start = |tx| provider.static_file_receipts_start_block(tx);

        let state = effective_resource(
            provider,
            &[PruneSegment::AccountHistory, PruneSegment::StorageHistory],
            &static_file_receipts_start,
        )?;
        let tx = effective_resource(
            provider,
            &[PruneSegment::TransactionLookup, PruneSegment::Bodies],
            &static_file_receipts_start,
        )?;
        let logs = effective_resource(
            provider,
            &[PruneSegment::Receipts, PruneSegment::ContractLogs],
            &static_file_receipts_start,
        )?;
        let receipts = effective_resource(
            provider,
            &[PruneSegment::Receipts, PruneSegment::TransactionLookup, PruneSegment::Bodies],
            &static_file_receipts_start,
        )?;
        let blocks =
            effective_resource(provider, &[PruneSegment::Bodies], &static_file_receipts_start)?;

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

/// Derives the effective block availability for a capability resource from the prune checkpoints of
/// all storage segments needed to serve it.
///
/// A resource is only available where every required segment is available, so the oldest available
/// block is the maximum pruned checkpoint across the segments. If any segment is configured with a
/// distance-based prune mode, or with full pruning that still keeps the segment minimum, expose the
/// smallest retention window because that is the tightest limit callers can rely on. If full
/// pruning leaves no reliable window for any required segment, the resource is disabled.
///
/// If the last transaction pruned by a receipts or contract logs checkpoint is stored in static
/// files, `static_file_receipts_start` returns the block from which receipts are stored there and
/// availability is limited by that block instead. Archive nodes that ran the removed
/// `StaticFileReceipts` pruner keep such a checkpoint for receipts that were moved to static files
/// rather than deleted.
///
/// See also: <https://github.com/ethereum/execution-apis/pull/755>
fn effective_resource(
    provider: &impl PruneCheckpointReader,
    segments: &[PruneSegment],
    static_file_receipts_start: &impl Fn(TxNumber) -> ProviderResult<Option<BlockNumber>>,
) -> RethResult<EthCapabilitiesResource> {
    let mut oldest_block = 0;
    let mut retention_blocks = None::<u64>;
    let mut disabled = false;

    for segment in segments {
        let Some(checkpoint) = provider.get_prune_checkpoint(*segment)? else {
            continue;
        };

        if let Some(block_number) = checkpoint.block_number {
            let mut available_from = block_number.saturating_add(1);
            if matches!(segment, PruneSegment::Receipts | PruneSegment::ContractLogs) &&
                let Some(tx_number) = checkpoint.tx_number &&
                let Some(start_block) = static_file_receipts_start(tx_number)?
            {
                available_from = available_from.min(start_block);
            }
            oldest_block = oldest_block.max(available_from);
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
    fn full_prune_segment_without_retention_disables_resource() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::TransactionLookup,
            PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full,
            },
        );

        let resource =
            effective_resource(&provider, &[PruneSegment::TransactionLookup], &|_| Ok(None))
                .unwrap();

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

        let resource =
            effective_resource(&provider, &[PruneSegment::Bodies], &|_| Ok(None)).unwrap();

        assert_eq!(
            resource,
            EthCapabilitiesResource::window(11, PruneSegment::Bodies.min_blocks())
        );
    }

    /// Archive nodes that ran the removed `StaticFileReceipts` pruner keep a checkpoint for
    /// receipts that were moved to static files, not deleted.
    fn receipts_moved_to_static_files(
        block_number: BlockNumber,
        tx_number: TxNumber,
    ) -> PruneCheckpoint {
        PruneCheckpoint {
            block_number: Some(block_number),
            tx_number: Some(tx_number),
            prune_mode: PruneMode::Before(block_number + 1),
        }
    }

    #[test]
    fn receipts_checkpoint_stored_in_static_files_uses_static_file_start() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Receipts,
            receipts_moved_to_static_files(143_318_608, 2_000_000_000),
        );

        let resource = effective_resource(
            &provider,
            &[PruneSegment::Receipts, PruneSegment::TransactionLookup, PruneSegment::Bodies],
            &|tx| Ok((tx == 2_000_000_000).then_some(0)),
        )
        .unwrap();

        assert_eq!(resource, EthCapabilitiesResource::available_from(0));
    }

    #[test]
    fn contract_logs_checkpoint_stored_in_static_files_uses_static_file_start() {
        let checkpoint = receipts_moved_to_static_files(143_318_608, 2_000_000_000);
        let provider = TestPruneCheckpointReader {
            checkpoints: HashMap::from([
                (PruneSegment::Receipts, checkpoint),
                (PruneSegment::ContractLogs, checkpoint),
            ]),
        };

        let resource = effective_resource(
            &provider,
            &[PruneSegment::Receipts, PruneSegment::ContractLogs],
            &|_| Ok(Some(105_235_063)),
        )
        .unwrap();

        assert_eq!(resource, EthCapabilitiesResource::available_from(105_235_063));
    }

    #[test]
    fn receipts_checkpoint_not_stored_in_static_files_keeps_checkpoint_floor() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Receipts,
            PruneCheckpoint {
                block_number: Some(499_999),
                tx_number: Some(1_000_000),
                prune_mode: PruneMode::Before(600_000),
            },
        );

        let resource =
            effective_resource(&provider, &[PruneSegment::Receipts], &|_| Ok(None)).unwrap();

        assert_eq!(resource, EthCapabilitiesResource::available_from(500_000));
    }

    #[test]
    fn receipts_checkpoint_without_tx_number_keeps_checkpoint_floor() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Receipts,
            PruneCheckpoint {
                block_number: Some(800_000),
                tx_number: None,
                prune_mode: PruneMode::Before(800_001),
            },
        );

        let resource =
            effective_resource(&provider, &[PruneSegment::Receipts], &|_| Ok(Some(0))).unwrap();

        assert_eq!(resource, EthCapabilitiesResource::available_from(800_001));
    }

    #[test]
    fn static_file_receipts_do_not_lower_other_segments() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Bodies,
            receipts_moved_to_static_files(800_000, 1_000_000),
        );

        let resource =
            effective_resource(&provider, &[PruneSegment::Bodies], &|_| Ok(Some(0))).unwrap();

        assert_eq!(resource, EthCapabilitiesResource::available_from(800_001));
    }

    #[test]
    fn distance_receipts_checkpoint_stored_in_static_files_keeps_window() {
        let provider = TestPruneCheckpointReader::with_checkpoint(
            PruneSegment::Receipts,
            PruneCheckpoint {
                block_number: Some(600_000),
                tx_number: Some(1_000_000),
                prune_mode: PruneMode::Distance(100_000),
            },
        );

        let resource =
            effective_resource(&provider, &[PruneSegment::Receipts], &|_| Ok(Some(0))).unwrap();

        assert_eq!(resource, EthCapabilitiesResource::window(0, 100_000));
    }
}
