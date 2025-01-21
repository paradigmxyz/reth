mod downloader;
pub use downloader::{fetch, Metadata};
use downloader::{DownloaderError, S3DownloaderResponse};

mod filelist;
use filelist::DOWNLOAD_FILE_LIST;

use reth_db::transaction::DbTxMut;
use reth_primitives::StaticFileSegment;
use reth_provider::{
    DBProvider, StageCheckpointReader, StageCheckpointWriter, StaticFileProviderFactory,
};
use reth_stages_api::{
    ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use std::{
    path::PathBuf,
    task::{ready, Context, Poll},
};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

/// S3 `StageId`
const S3_STAGE_ID: StageId = StageId::Other("S3");

/// The S3 stage.
#[derive(Default, Debug)]
#[non_exhaustive]
pub struct S3Stage {
    /// Static file directory.
    static_file_directory: PathBuf,
    /// Remote server URL.
    url: String,
    /// Maximum number of connections per download.
    max_concurrent_requests: u64,
    /// Channel to receive the downloaded ranges from the fetch task.
    fetch_rx: Option<UnboundedReceiver<Result<S3DownloaderResponse, DownloaderError>>>,
}

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

    fn poll_execute_ready(
        &mut self,
        cx: &mut Context<'_>,
        input: ExecInput,
    ) -> Poll<Result<(), StageError>> {
        loop {
            // We are currently fetching and may have downloaded ranges that we can process.
            if let Some(rx) = &mut self.fetch_rx {
                // Whether we have downloaded all the required files.
                let mut is_done = false;

                let response = match ready!(rx.poll_recv(cx)) {
                    Some(Ok(response)) => {
                        is_done = response.is_done();
                        Ok(())
                    }
                    Some(Err(_)) => todo!(), // TODO: DownloaderError -> StageError
                    None => Err(StageError::ChannelClosed),
                };

                if is_done {
                    self.fetch_rx = None;
                }

                return Poll::Ready(response)
            }

            // Spawns the downloader task if there are any missing files
            if let Some(fetch_rx) = self.maybe_spawn_fetch(input) {
                self.fetch_rx = Some(fetch_rx);

                // Polls fetch_rx & registers waker
                continue
            }

            // No files to be downloaded
            return Poll::Ready(Ok(()))
        }
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError>
    where
        Provider: DBProvider<Tx: DbTxMut>
            + StaticFileProviderFactory
            + StageCheckpointReader
            + StageCheckpointWriter,
    {
        // Re-initializes the provider to detect the new additions
        provider.static_file_provider().initialize_index()?;

        // TODO logic for appending tx_block

        // let (_, _to_block) = input.next_block_range().into_inner();
        // let static_file_provider = provider.static_file_provider();
        // let mut _tx_block_cursor =
        // provider.tx_ref().cursor_write::<tables::TransactionBlocks>()?;

        // tx_block_cursor.append(indice.last_tx_num(), &block_number)?;

        // let checkpoint = StageCheckpoint { block_number: highest_block, stage_checkpoint: None };
        // provider.save_stage_checkpoint(StageId::Bodies, checkpoint)?;
        // provider.save_stage_checkpoint(S3_STAGE_ID, checkpoint)?;

        // // TODO: verify input.target according to s3 stage specifications
        // let done = highest_block == to_block;

        Ok(ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true })
    }

    fn unwind(
        &mut self,
        _provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // TODO
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}

impl S3Stage {
    /// It will only spawn a task to fetch files from the remote server, it there are any missing
    /// static files.
    ///
    /// Every time a block range is ready with all the necessary files, it sends a
    /// [`S3DownloaderResponse`] to `self.fetch_rx`. If it's the last requested block range, the
    /// response will have `is_done` set to true.
    fn maybe_spawn_fetch(
        &self,
        input: ExecInput,
    ) -> Option<UnboundedReceiver<Result<S3DownloaderResponse, DownloaderError>>> {
        let checkpoint = input.checkpoint();
        // TODO: input target can only be certain numbers. eg. 499_999 , 999_999 etc.

        // Create a list of all the missing files per block range that need to be downloaded.
        let mut requests = vec![];
        for block_range_files in &DOWNLOAD_FILE_LIST {
            let (_, block_range) =
                StaticFileSegment::parse_filename(block_range_files[0].0).expect("qed");

            if block_range.end() <= checkpoint.block_number {
                continue
            }

            let mut block_range_requests = vec![];
            for (filename, file_hash) in block_range_files {
                // If the file already exists, then we are resuming a previously interrupted stage
                // run.
                if self.static_file_directory.join(filename).exists() {
                    // TODO: check hash if the file already exists
                    continue
                }

                block_range_requests.push((filename, file_hash));
            }

            requests.push((block_range, block_range_requests));
        }

        // Return None, if we have downloaded all the files that are required.
        if requests.is_empty() {
            return None
        }

        let static_file_directory = self.static_file_directory.clone();
        let url = self.url.clone();
        let max_concurrent_requests = self.max_concurrent_requests;

        let (fetch_tx, fetch_rx) = unbounded_channel();
        tokio::spawn(async move {
            let mut requests_iter = requests.into_iter().peekable();

            while let Some((_, file_requests)) = requests_iter.next() {
                for (filename, file_hash) in file_requests {
                    if let Err(err) = fetch(
                        filename,
                        &static_file_directory,
                        &format!("{}/{filename}", url),
                        max_concurrent_requests,
                        Some(*file_hash),
                    )
                    .await
                    {
                        let _ = fetch_tx.send(Err(err));
                        return
                    }
                }

                let response = if requests_iter.peek().is_none() {
                    S3DownloaderResponse::Done
                } else {
                    S3DownloaderResponse::AddedNewRange
                };

                let _ = fetch_tx.send(Ok(response));
            }
        });

        Some(fetch_rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        ExecuteStageTestRunner, StageTestRunner, TestRunnerError, TestStageDB,
        UnwindStageTestRunner,
    };
    use reth_primitives::SealedHeader;
    use reth_testing_utils::{
        generators,
        generators::{random_header, random_header_range},
    };

    // stage_test_suite_ext!(S3TestRunner, s3);

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
            S3Stage::default()
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
