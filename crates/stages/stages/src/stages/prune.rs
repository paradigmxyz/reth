use reth_db::transaction::DbTxMut;
use reth_provider::{
    BlockReader, DBProvider, PruneCheckpointReader, PruneCheckpointWriter,
    StaticFileProviderFactory,
};
use reth_prune::{
    PruneMode, PruneModes, PruneSegment, PrunerBuilder, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
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
#[derive(Debug)]
pub struct PruneStage {
    prune_modes: PruneModes,
    commit_threshold: usize,
}

impl PruneStage {
    /// Crate new prune stage with the given prune modes and commit threshold.
    pub const fn new(prune_modes: PruneModes, commit_threshold: usize) -> Self {
        Self { prune_modes, commit_threshold }
    }
}

impl<Provider> Stage<Provider> for PruneStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + BlockReader
        + StaticFileProviderFactory,
{
    fn id(&self) -> StageId {
        StageId::Prune
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        let mut pruner = PrunerBuilder::default()
            .segments(self.prune_modes.clone())
            .delete_limit(self.commit_threshold)
            .build::<Provider>(provider.static_file_provider());

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
        for (segment, mut checkpoint) in prune_checkpoints {
            checkpoint.block_number = Some(input.unwind_to);
            provider.save_prune_checkpoint(segment, checkpoint)?;
        }
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

/// The prune sender recovery stage that runs the pruner with the provided `PruneMode` for the
/// `SenderRecovery` segment.
///
/// Under the hood, this stage has the same functionality as [`PruneStage`].
#[derive(Debug)]
pub struct PruneSenderRecoveryStage(PruneStage);

impl PruneSenderRecoveryStage {
    /// Create new prune sender recovery stage with the given prune mode and commit threshold.
    pub fn new(prune_mode: PruneMode, commit_threshold: usize) -> Self {
        Self(PruneStage::new(
            PruneModes { sender_recovery: Some(prune_mode), ..PruneModes::none() },
            commit_threshold,
        ))
    }
}

impl<Provider> Stage<Provider> for PruneSenderRecoveryStage
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + BlockReader
        + StaticFileProviderFactory,
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
        TestRunnerError, TestStageDB, UnwindStageTestRunner,
    };
    use alloy_primitives::B256;
    use reth_primitives::SealedBlock;
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
        type S = PruneStage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            PruneStage {
                prune_modes: PruneModes {
                    sender_recovery: Some(PruneMode::Full),
                    ..Default::default()
                },
                commit_threshold: usize::MAX,
            }
        }
    }

    impl ExecuteStageTestRunner for PruneTestRunner {
        type Seed = Vec<SealedBlock>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let mut rng = generators::rng();
            let blocks = random_block_range(
                &mut rng,
                input.checkpoint().block_number..=input.target(),
                BlockRangeParams { parent: Some(B256::ZERO), tx_count: 1..3, ..Default::default() },
            );
            self.db.insert_blocks(blocks.iter(), StorageKind::Static)?;
            self.db.insert_transaction_senders(
                blocks.iter().flat_map(|block| block.body.transactions.iter()).enumerate().map(
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
}
