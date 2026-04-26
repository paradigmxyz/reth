use super::{
    extract::{extract_archive_raw, streaming_download_and_extract, CompressionFormat},
    fetch::ArchiveFetcher,
    manifest::SnapshotArchive,
    planning::{PlannedArchive, PlannedDownloads},
    progress::{
        spawn_progress_display, ArchiveDownloadProgress, ArchiveExtractionProgress,
        ArchiveVerificationProgress, DownloadRequestLimiter, SharedProgress,
    },
    session::{ArchiveProcessContext, DownloadSession},
    verify::OutputVerifier,
    MAX_DOWNLOAD_RETRIES, RETRY_BACKOFF_SECS,
};
use eyre::Result;
use futures::stream::{self, StreamExt};
use reth_cli_util::cancellation::CancellationToken;
use reth_fs_util as fs;
use std::{
    path::Path,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::task;
use tracing::{debug, info, warn};

const DOWNLOAD_CACHE_DIR: &str = ".download-cache";

/// Runs all planned modular archive downloads for one command invocation.
pub(crate) async fn run_modular_downloads(
    planned_downloads: PlannedDownloads,
    target_dir: &Path,
    download_concurrency: usize,
    cancel_token: CancellationToken,
) -> Result<()> {
    let download_cache_dir = target_dir.join(DOWNLOAD_CACHE_DIR);
    fs::create_dir_all(&download_cache_dir)?;

    let shared = SharedProgress::new(
        planned_downloads.total_download_size,
        planned_downloads.total_output_size,
        planned_downloads.total_archives() as u64,
        cancel_token.clone(),
    );
    let session = DownloadSession::new(
        Some(Arc::clone(&shared)),
        Some(DownloadRequestLimiter::new(download_concurrency)),
        cancel_token,
    );
    let ctx =
        ArchiveProcessContext::new(target_dir.to_path_buf(), Some(download_cache_dir), session);

    ModularDownloadJob::new(ctx, download_concurrency).run(planned_downloads).await
}

/// Schedules modular archive work for one run of `reth download`.
struct ModularDownloadJob {
    /// Shared paths and session state for each archive in this job.
    ctx: ArchiveProcessContext,
    /// Maximum number of archives processed at once.
    archive_concurrency: usize,
}

impl ModularDownloadJob {
    /// Creates the modular download job for one command run.
    const fn new(ctx: ArchiveProcessContext, archive_concurrency: usize) -> Self {
        Self { ctx, archive_concurrency }
    }

    /// Runs all planned archives and waits for the shared progress task to finish.
    async fn run(self, planned_downloads: PlannedDownloads) -> Result<()> {
        let shared = Arc::clone(
            self.ctx.session().progress().expect("modular downloads always use shared progress"),
        );
        let progress_handle = spawn_progress_display(Arc::clone(&shared));
        let ctx = self.ctx.clone();
        let results: Vec<Result<()>> = stream::iter(planned_downloads.archives)
            .map(move |archive| {
                let ctx = ctx.clone();
                async move { Self::process_archive(ctx, archive).await }
            })
            .buffer_unordered(self.archive_concurrency)
            .collect()
            .await;

        shared.done.store(true, Ordering::Relaxed);
        let _ = progress_handle.await;

        for result in results {
            result?;
        }

        Ok(())
    }

    /// Runs one archive on the blocking pool so fetch and extraction stay off the async executor.
    async fn process_archive(ctx: ArchiveProcessContext, archive: PlannedArchive) -> Result<()> {
        task::spawn_blocking(move || ArchiveProcessor::new(archive, ctx).run()).await??;
        Ok(())
    }
}

/// Explicit retry states for one modular archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveAttemptState {
    /// Start or restart one full archive attempt.
    RunAttempt,
    /// Check whether the extracted outputs verify.
    VerifyOutputs,
    /// Wait and decide whether another full attempt should run.
    RetryAttempt,
    /// Finish successfully.
    Complete,
    /// Stop with an error after retries are exhausted.
    Fail,
}

/// Processes one modular archive from reuse check through extraction and verification.
struct ArchiveProcessor {
    /// The concrete archive and component being processed.
    archive: PlannedArchive,
    /// Shared paths and session state for this archive attempt.
    ctx: ArchiveProcessContext,
}

impl ArchiveProcessor {
    /// Creates a processor for one archive and the shared download context.
    fn new(archive: PlannedArchive, ctx: ArchiveProcessContext) -> Self {
        Self { archive, ctx }
    }

    /// Runs the archive retry state machine until outputs are verified or retries are exhausted.
    fn run(self) -> Result<()> {
        let archive = self.archive();
        if self.try_reuse_outputs()? {
            info!(target: "reth::cli", file = %archive.file_name, component = %self.archive.component, "Skipping already verified plain files");
            return Ok(());
        }

        let mode = ArchiveMode::new(&self.ctx)?;
        let format = CompressionFormat::from_url(&archive.file_name)?;
        let mut attempt = 1;
        let mut last_error: Option<eyre::Error> = None;
        let mut state = ArchiveAttemptState::RunAttempt;

        loop {
            match state {
                ArchiveAttemptState::RunAttempt => {
                    self.cleanup_outputs();

                    if attempt > 1 {
                        info!(target: "reth::cli",
                            file = %archive.file_name,
                            component = %self.archive.component,
                            attempt,
                            max = MAX_DOWNLOAD_RETRIES,
                            "Retrying archive from scratch"
                        );
                    }

                    match self.run_attempt(mode, format) {
                        Ok(()) => state = ArchiveAttemptState::VerifyOutputs,
                        Err(error) if mode.retries_fetch_errors() => {
                            warn!(target: "reth::cli",
                                file = %archive.file_name,
                                component = %self.archive.component,
                                attempt,
                                err = %format_args!("{error:#}"),
                                "Archive attempt failed, retrying from scratch"
                            );
                            last_error = Some(error);
                            state = ArchiveAttemptState::RetryAttempt;
                        }
                        Err(error) => return Err(error),
                    }
                }
                ArchiveAttemptState::VerifyOutputs => {
                    if self.verify_outputs_with_progress()? {
                        state = ArchiveAttemptState::Complete;
                    } else {
                        warn!(target: "reth::cli", file = %archive.file_name, component = %self.archive.component, attempt, "Archive extracted, but output verification failed, retrying");
                        state = ArchiveAttemptState::RetryAttempt;
                    }
                }
                ArchiveAttemptState::RetryAttempt => {
                    if attempt >= MAX_DOWNLOAD_RETRIES {
                        state = ArchiveAttemptState::Fail;
                    } else {
                        std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                        attempt += 1;
                        state = ArchiveAttemptState::RunAttempt;
                    }
                }
                ArchiveAttemptState::Complete => return Ok(()),
                ArchiveAttemptState::Fail => {
                    if let Some(error) = last_error {
                        return Err(error.wrap_err(format!(
                            "Failed after {} attempts for {}",
                            MAX_DOWNLOAD_RETRIES, archive.file_name
                        )));
                    }

                    eyre::bail!(
                        "Failed integrity validation after {} attempts for {}",
                        MAX_DOWNLOAD_RETRIES,
                        archive.file_name
                    )
                }
            }
        }
    }

    /// Returns the concrete archive being fetched or verified.
    fn archive(&self) -> &SnapshotArchive {
        &self.archive.archive
    }

    /// Returns the verifier for this archive's output files.
    fn output_verifier(&self) -> OutputVerifier<'_> {
        OutputVerifier::new(self.ctx.target_dir())
    }

    /// Returns `true` if this archive can be reused from existing verified outputs.
    /// Returns `false` if a fresh archive attempt is still needed.
    fn try_reuse_outputs(&self) -> Result<bool> {
        if self.verify_outputs()? {
            self.mark_complete();
            return Ok(true);
        }

        Ok(false)
    }

    /// Removes any partial outputs before a fresh archive attempt.
    fn cleanup_outputs(&self) {
        self.output_verifier().cleanup(&self.archive().output_files);
    }

    /// Returns `true` if all declared plain outputs verify.
    /// Returns `false` if any output is missing or does not match.
    fn verify_outputs(&self) -> Result<bool> {
        self.output_verifier().verify(&self.archive().output_files)
    }

    /// Records archive completion in shared progress once outputs verify.
    fn mark_complete(&self) {
        self.ctx.session().record_reused_archive(self.archive().size, self.archive().output_size());
    }

    /// Executes one archive attempt according to the selected cache-vs-stream mode.
    fn run_attempt(&self, mode: ArchiveMode, format: CompressionFormat) -> Result<()> {
        mode.execute(self, format)
    }

    /// Downloads the archive into the cache, then extracts from the cached file.
    fn run_cached_attempt(&self, format: CompressionFormat) -> Result<()> {
        let cache_dir =
            self.ctx.cache_dir().ok_or_else(|| eyre::eyre!("Missing download cache directory"))?;
        let fetcher =
            ArchiveFetcher::new(self.archive().url.clone(), cache_dir, self.ctx.session().clone());

        if self.archive.ty == super::manifest::SnapshotComponentType::State {
            debug!(target: "reth::cli", url = %self.archive().url, "Downloading state snapshot archive");
        }

        let download_result = {
            let mut download_progress = ArchiveDownloadProgress::new(self.ctx.session().progress());
            let result = fetcher.download(Some(&mut download_progress));
            if let Ok(ref downloaded) = result &&
                download_progress.has_tracked_bytes()
            {
                download_progress.complete(downloaded.size);
            }
            result
        };

        let downloaded = match download_result {
            Ok(downloaded) => downloaded,
            Err(error) => {
                fetcher.cleanup_downloaded_files();
                return Err(error);
            }
        };

        info!(target: "reth::cli",
            file = %self.archive().file_name,
            component = %self.archive.component,
            size = %super::progress::DownloadProgress::format_size(downloaded.size),
            "Archive download complete"
        );

        let extract_result = self.extract_cached_archive(&downloaded.path, format);
        fetcher.cleanup_downloaded_files();
        extract_result
    }

    /// Streams the archive directly into extraction without keeping a cached copy.
    fn run_streaming_attempt(&self, format: CompressionFormat) -> Result<()> {
        let _download_progress = ArchiveDownloadProgress::new(self.ctx.session().progress());
        streaming_download_and_extract(
            &self.archive().url,
            format,
            self.ctx.target_dir(),
            self.ctx.session(),
        )
    }

    /// Extracts a cached archive file while updating shared extraction activity.
    fn extract_cached_archive(&self, archive_path: &Path, format: CompressionFormat) -> Result<()> {
        let mut extraction_progress = ArchiveExtractionProgress::new(self.ctx.session().progress());
        let file = fs::open(archive_path)?;
        let result = extract_archive_raw(
            file,
            format,
            self.ctx.target_dir(),
            Some(&mut extraction_progress),
        );
        extraction_progress.finish();
        result
    }

    /// Returns `true` if all declared plain outputs verify while updating shared verification
    /// progress.
    fn verify_outputs_with_progress(&self) -> Result<bool> {
        let mut verification_progress =
            ArchiveVerificationProgress::new(self.ctx.session().progress());
        let verified = self
            .output_verifier()
            .verify_with_progress(&self.archive().output_files, Some(&mut verification_progress))?;
        if verified {
            verification_progress.complete(self.archive().output_size());
        }
        Ok(verified)
    }
}

/// Chooses whether an archive attempt uses the cache or streams directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveMode {
    /// Download the archive to the cache, then extract it.
    Cached,
    /// Stream the archive directly into extraction.
    Streaming,
}

impl ArchiveMode {
    /// Picks the archive mode from the process context.
    fn new(ctx: &ArchiveProcessContext) -> Result<Self> {
        if ctx.cache_dir().is_some() {
            ctx.session().require_request_limiter()?;
            return Ok(Self::Cached)
        }

        Ok(Self::Streaming)
    }

    /// Returns `true` when fetch failures should retry the whole archive attempt.
    const fn retries_fetch_errors(&self) -> bool {
        matches!(self, Self::Cached)
    }

    /// Runs the selected archive mode for a single attempt.
    fn execute(&self, processor: &ArchiveProcessor, format: CompressionFormat) -> Result<()> {
        match self {
            Self::Cached => processor.run_cached_attempt(format),
            Self::Streaming => processor.run_streaming_attempt(format),
        }
    }
}
