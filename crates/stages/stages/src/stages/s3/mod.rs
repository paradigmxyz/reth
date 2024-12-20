mod downloader;
use downloader::fetch;

mod filelist;
use filelist::DOWNLOAD_FILE_LIST;

use reth_db::{cursor::DbCursorRW, tables, transaction::DbTxMut};
use reth_primitives::StaticFileSegment;
use reth_provider::{
    BlockBodyIndicesProvider, DBProvider, StageCheckpointReader, StageCheckpointWriter,
    StaticFileProviderFactory,
};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};

/// S3 `StageId`
const S3_STAGE_ID: StageId = StageId::Other("S3");

/// The s3 stage.
///
/// This stage does not write anything; it's checkpoint is used to denote the highest fully synced
/// block.
#[derive(Default, Debug, Clone)]
#[non_exhaustive]
pub struct S3Stage;

impl<Provider> Stage<Provider> for S3Stage
where
    Provider: DBProvider<Tx: DbTxMut>
        + StaticFileProviderFactory
        + StageCheckpointReader
        + StageCheckpointWriter,
{
    fn id(&self) -> StageId {
        S3_STAGE_ID
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError>
    where
        Provider: DBProvider<Tx: DbTxMut>
            + StaticFileProviderFactory
            + StageCheckpointReader
            + StageCheckpointWriter,
    {
        // TODO: validate input.target() has to be *999
        // TODO: MAKE CONFIG URL
        let cfg = "localhost:8000";
        let checkpoint = provider.get_stage_checkpoint(S3_STAGE_ID)?.unwrap_or_default();
        let static_file_provider = provider.static_file_provider();
        let mut tx_block_cursor = provider.tx_ref().cursor_write::<tables::TransactionBlocks>()?;

        for block_range_files in &DOWNLOAD_FILE_LIST {
            let (_, block_range) =
                StaticFileSegment::parse_filename(block_range_files[0].0).expect("qed");

            if block_range.end() <= checkpoint.block_number {
                continue
            }

            for (filename, file_hash) in block_range_files {
                // It already exists.
                if static_file_provider.directory().join(filename).exists() {
                    // TODO: check hash?
                    continue
                }

                fetch(
                    filename,
                    static_file_provider.directory(),
                    &format!("{cfg}/{filename}"),
                    std::thread::available_parallelism()?.get() as u64,
                    Some(*file_hash),
                )
                .unwrap();
            }

            // Re-initializes the provider to detect the new additions
            static_file_provider.initialize_index()?;

            // Populate TransactionBlock table
            for block_number in block_range.start()..=block_range.end() {
                // TODO: should be error if none tbh
                if let Some(indice) = static_file_provider.block_body_indices(block_number)? {
                    if indice.tx_count() > 0 {
                        tx_block_cursor.append(indice.last_tx_num(), block_number)?;
                    }
                }
            }

            let checkpoint =
                StageCheckpoint { block_number: block_range.end(), stage_checkpoint: None };
            provider.save_stage_checkpoint(StageId::Bodies, checkpoint)?;
            provider.save_stage_checkpoint(S3_STAGE_ID, checkpoint)?;
        }

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true })
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Todo
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        stage_test_suite_ext, ExecuteStageTestRunner, StageTestRunner, TestRunnerError,
        TestStageDB, UnwindStageTestRunner,
    };
    use reth_primitives::SealedHeader;
    use reth_provider::providers::StaticFileWriter;
    use reth_testing_utils::{
        generators,
        generators::{random_header, random_header_range},
    };

    stage_test_suite_ext!(S3TestRunner, s3);

    #[derive(Default)]
    struct S3TestRunner {
        db: TestStageDB,
    }

    impl StageTestRunner for S3TestRunner {
        type S = S3Stage;

        fn db(&self) -> &TestStageDB {
            &self.db
        }

        fn stage(&self) -> Self::S {
            S3Stage
        }
    }

    impl ExecuteStageTestRunner for S3TestRunner {
        type Seed = Vec<SealedHeader>;

        fn seed_execution(&mut self, input: ExecInput) -> Result<Self::Seed, TestRunnerError> {
            let start = input.checkpoint().block_number;
            let mut rng = generators::rng();
            let head = random_header(&mut rng, start, None);
            self.db.insert_headers_with_td(std::iter::once(&head))?;

            // use previous progress as seed size
            let end = input.target.unwrap_or_default() + 1;

            if start + 1 >= end {
                return Ok(Vec::default())
            }

            let mut headers = random_header_range(&mut rng, start + 1..end, head.hash());
            self.db.insert_headers_with_td(headers.iter())?;
            headers.insert(0, head);
            Ok(headers)
        }

        fn validate_execution(
            &self,
            input: ExecInput,
            output: Option<ExecOutput>,
        ) -> Result<(), TestRunnerError> {
            if let Some(output) = output {
                assert!(output.done, "stage should always be done");
                assert_eq!(
                    output.checkpoint.block_number,
                    input.target(),
                    "stage progress should always match progress of previous stage"
                );
            }
            Ok(())
        }
    }

    impl UnwindStageTestRunner for S3TestRunner {
        fn validate_unwind(&self, _input: UnwindInput) -> Result<(), TestRunnerError> {
            Ok(())
        }
    }

    #[test]
    fn parse_files() {
        for block_range_files in &DOWNLOAD_FILE_LIST {
            let (_, _) = StaticFileSegment::parse_filename(block_range_files[0].0).expect("qed");
        }
    }
}
