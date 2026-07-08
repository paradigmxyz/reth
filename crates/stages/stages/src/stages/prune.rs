use reth_db_api::{table::Value, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    BlockReader, ChainStateBlockReader, DBProvider, PruneCheckpointReader, PruneCheckpointWriter,
    RocksDBProviderFactory, StageCheckpointReader, StaticFileProviderFactory,
};
use reth_prune::{
    segments::Segment, PruneMode, PruneModes, PruneSegment, PrunerBuilder, SegmentOutput,
    SegmentOutputCheckpoint,
};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader, StorageSettingsCache};
use std::{fmt, sync::Arc};
use tracing::info;

/// The prune stage that runs the pruner with the provided prune modes.
///
/// There are two main reasons to have this stage when running a full node:
/// - Sender Recovery stage inserts a lot of data into the database that's only needed for the
///   Execution stage. Pruner will clean up the unneeded recovered senders.
/// - Pruning during the live sync can take a significant amount of time, especially history
///   segments. If we can prune as much data as possible in one go before starting the live sync, we
///   should do it.
///
/// `commit_threshold` is the maximum number of entries to prune before committing
/// progress to the database.
pub struct PruneStage<Provider> {
    prune_modes: PruneModes,
    commit_threshold: usize,
    /// Additional segments to prune besides the built-in ones, e.g. custom segments defined by a
    /// downstream node, see [`PruneSegment::Custom`].
    ///
    /// Segments are `Arc`ed so a single instance can be shared with the pruner that runs during
    /// live sync.
    custom_segments: Vec<Arc<dyn Segment<Provider>>>,
}

impl<Provider> PruneStage<Provider> {
    /// Crate new prune stage with the given prune modes and commit threshold.
    pub const fn new(prune_modes: PruneModes, commit_threshold: usize) -> Self {
        Self { prune_modes, commit_threshold, custom_segments: Vec::new() }
    }

    /// Appends additional segments to prune besides the built-in ones, e.g. custom segments
    /// defined by a downstream node, see [`PruneSegment::Custom`].
    pub fn with_custom_segments(
        mut self,
        segments: impl IntoIterator<Item = Arc<dyn Segment<Provider>>>,
    ) -> Self {
        self.custom_segments.extend(segments);
        self
    }
}

// Not derived to avoid requiring `Provider: Debug`, which the stage only uses as the `dyn
// Segment` parameter.
impl<Provider> fmt::Debug for PruneStage<Provider> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PruneStage")
            .field("prune_modes", &self.prune_modes)
            .field("commit_threshold", &self.commit_threshold)
            .field("custom_segments", &self.custom_segments)
            .finish()
    }
}

impl<Provider> Stage<Provider> for PruneStage<Provider>
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + BlockReader
        + ChainStateBlockReader
        + StageCheckpointReader
        + StaticFileProviderFactory<
            Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
        > + StorageSettingsCache
        + ChangeSetReader
        + StorageChangeSetReader
        + RocksDBProviderFactory
        // `'static` is required to pass the custom `dyn Segment<Provider>` segments to the pruner
        + 'static,
{
    fn id(&self) -> StageId {
        StageId::Prune
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let mut pruner = PrunerBuilder::default()
            .segments(self.prune_modes.clone())
            .delete_limit(self.commit_threshold)
            .build::<Provider>(provider.static_file_provider());
        pruner.extend_segments(self.custom_segments.iter().cloned());

        let result = pruner.run_with_provider(provider, input.target())?;
        if result.progress.is_finished() {
            Ok(ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true })
        } else {
            if let Some((last_segment, last_segment_output)) = result.segments.last() {
                match last_segment_output {
                    SegmentOutput {
                        progress,
                        pruned,
                        checkpoint:
                            checkpoint @ Some(SegmentOutputCheckpoint { block_number: Some(_), .. }),
                    } => {
                        info!(
                            target: "sync::stages::prune::exec",
                            ?last_segment,
                            ?progress,
                            ?pruned,
                            ?checkpoint,
                            "Last segment has more data to prune"
                        )
                    }
                    SegmentOutput { progress, pruned, checkpoint: _ } => {
                        info!(
                            target: "sync::stages::prune::exec",
                            ?last_segment,
                            ?progress,
                            ?pruned,
                            "Last segment has more data to prune"
                        )
                    }
                }
            }
            // We cannot set the checkpoint yet, because prune segments may have different highest
            // pruned block numbers
            Ok(ExecOutput { checkpoint: input.checkpoint(), done: false })
        }
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // We cannot recover the data that was pruned in `execute`, so we just update the
        // checkpoints.
        let prune_checkpoints = provider.get_prune_checkpoints()?;
        let unwind_to_last_tx =
            provider.block_body_indices(input.unwind_to)?.map(|i| i.last_tx_num());

        for (segment, mut checkpoint) in prune_checkpoints {
            // Only update the checkpoint if unwind_to is lower than the existing checkpoint.
            if let Some(block) = checkpoint.block_number &&
                input.unwind_to < block
            {
                checkpoint.block_number = Some(input.unwind_to);
                checkpoint.tx_number = unwind_to_last_tx;
                provider.save_prune_checkpoint(segment, checkpoint)?;
            }
        }
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

/// The prune sender recovery stage that runs the pruner with the provided `PruneMode` for the
/// `SenderRecovery` segment.
///
/// Under the hood, this stage has the same functionality as [`PruneStage`].
///
/// Should be run right after `Execution`, unlike [`PruneStage`] which runs at the end.
/// This lets subsequent stages reuse the freed pages instead of growing the freelist.
pub struct PruneSenderRecoveryStage<Provider>(PruneStage<Provider>);

// Not derived to avoid requiring `Provider: Debug`, see [`PruneStage`]'s `Debug` impl.
impl<Provider> fmt::Debug for PruneSenderRecoveryStage<Provider> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PruneSenderRecoveryStage").field(&self.0).finish()
    }
}

impl<Provider> PruneSenderRecoveryStage<Provider> {
    /// Create new prune sender recovery stage with the given prune mode and commit threshold.
    pub fn new(prune_mode: PruneMode, commit_threshold: usize) -> Self {
        Self(PruneStage::new(
            PruneModes { sender_recovery: Some(prune_mode), ..PruneModes::default() },
            commit_threshold,
        ))
    }
}

impl<Provider> Stage<Provider> for PruneSenderRecoveryStage<Provider>
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + BlockReader
        + ChainStateBlockReader
        + StageCheckpointReader
        + StaticFileProviderFactory<
            Primitives: NodePrimitives<SignedTx: Value, Receipt: Value, BlockHeader: Value>,
        > + StorageSettingsCache
        + ChangeSetReader
        + StorageChangeSetReader
        + RocksDBProviderFactory
        + 'static,
{
    fn id(&self) -> StageId {
        StageId::PruneSenderRecovery
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let mut result = self.0.execute(provider, input)?;

        // Adjust the checkpoint to the highest pruned block number of the Sender Recovery segment
        if !result.done {
            let checkpoint = provider
                .get_prune_checkpoint(PruneSegment::SenderRecovery)?
                .ok_or(StageError::MissingPruneCheckpoint(PruneSegment::SenderRecovery))?;

            // `unwrap_or_default` is safe because we know that genesis block doesn't have any
            // transactions and senders
            result.checkpoint = StageCheckpoint::new(checkpoint.block_number.unwrap_or_default());
        }

        Ok(result)
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        self.0.unwind(provider, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, StorageKind,
        TestRunnerError, TestStageDB, TestStageProvider, UnwindStageTestRunner,
    };
    use alloy_primitives::B256;
    use reth_ethereum_primitives::Block;
    use reth_primitives_traits::{SealedBlock, SignerRecoverable};
    use reth_provider::{
        providers::StaticFileWriter, TransactionsProvider, TransactionsProviderExt,
    };
    use reth_prune::PruneMode;
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

    stage_test_suite_ext!(PruneTestRunner, prune);

    #[derive(Default)]
    struct PruneTestRunner {
        db: TestStageDB,
    }

    impl StageTestRunner for PruneTestRunner {
        type S = PruneStage<TestStageProvider>;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            PruneStage::new(
                PruneModes { sender_recovery: Some(PruneMode::Full), ..Default::default() },
                usize::MAX,
            )
        }
    }

    impl ExecuteStageTestRunner for PruneTestRunner {
        type Seed = Vec<SealedBlock<Block>>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let mut rng = generators::rng();
            let blocks = random_block_range(
                &mut rng,
                input.checkpoint().block_number..=input.target(),
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 1..3, ..Default::default() },
            );
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;
            self.db.insert_transaction_senders(
                blocks.iter().flat_map(|block| block.body().transactions.iter()).enumerate().map(
                    |(i, tx)| (i as u64, tx.recover_signer().expect("failed to recover signer")),
                ),
            )?;
            Ok(blocks)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                let start_block = input.next_block();
                let end_block = output.checkpoint.block_number;

                if start_block > end_block {
                    return Ok(())
                }

                let provider = self.db.factory.provider()?;

                assert!(output.done);
                assert_eq!(
                    output.checkpoint.block_number,
                    provider
                        .get_prune_checkpoint(PruneSegment::SenderRecovery)?
                        .expect("prune checkpoint must exist")
                        .block_number
                        .unwrap_or_default()
                );

                // Verify that the senders are pruned
                let tx_range =
                    provider.transaction_range_by_block_range(start_block..=end_block)?;
                let senders = self.db.factory.provider()?.senders_by_tx_range(tx_range)?;
                assert!(senders.is_empty());
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for PruneTestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            Ok(())
        }
    }

    /// A user-defined segment, as a downstream node would install via the node builder: prunes
    /// nothing, but records checkpoints under [`PruneSegment::Custom`].
    #[derive(Debug)]
    struct CustomSegment;

    impl<Provider> Segment<Provider> for CustomSegment {
        fn segment(&self) -> PruneSegment {
            PruneSegment::Custom(7)
        }

        fn mode(&self) -> Option<PruneMode> {
            Some(PruneMode::Full)
        }

        fn purpose(&self) -> reth_prune::PrunePurpose {
            reth_prune::PrunePurpose::User
        }

        fn min_blocks(&self) -> Option<u64> {
            Some(0)
        }

        fn prune(
            &self,
            _provider: &Provider,
            input: reth_prune::segments::PruneInput,
        ) -> Result<SegmentOutput, reth_prune::PrunerError> {
            Ok(SegmentOutput {
                progress: reth_prune::PruneProgress::Finished,
                pruned: 0,
                checkpoint: Some(SegmentOutputCheckpoint {
                    block_number: Some(input.to_block),
                    tx_number: None,
                }),
            })
        }
    }

    #[test]
    fn prune_stage_runs_custom_segments() {
        use reth_provider::DatabaseProviderFactory;
        use reth_prune::PruneCheckpoint;
        use std::sync::Arc;

        let db = TestStageDB::default();
        let mut stage = PruneStage::new(PruneModes::default(), usize::MAX)
            .with_custom_segments([Arc::new(CustomSegment) as Arc<dyn Segment<_>>]);

        let provider = db.factory.database_provider_rw().unwrap();
        let input = ExecInput { target: Some(10), checkpoint: None };
        let output = stage.execute(&provider, input).expect("stage execution");
        assert!(output.done);

        // The custom segment's checkpoint is persisted under its custom segment key
        assert_eq!(
            provider.get_prune_checkpoint(PruneSegment::Custom(7)).unwrap(),
            Some(PruneCheckpoint {
                block_number: Some(10),
                tx_number: None,
                prune_mode: PruneMode::Full
            })
        );
    }
}
