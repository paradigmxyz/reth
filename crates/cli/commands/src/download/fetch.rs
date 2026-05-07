use super::{
    progress::{
        ArchiveDownloadProgress, DownloadProgress, DownloadRequestLimiter, SharedProgress,
        SharedProgressWriter,
    },
    session::DownloadSession,
    RETRY_BACKOFF_SECS,
};
use eyre::Result;
use reqwest::{blocking::Client as BlockingClient, header::RANGE, StatusCode};
use reth_cli_util::cancellation::CancellationToken;
use reth_fs_util as fs;
use std::{
    any::Any,
    collections::VecDeque,
    fs::OpenOptions,
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tracing::info;
use url::Url;

/// Maximum retry attempts for a single download segment.
const SEGMENT_RETRY_ATTEMPTS: u32 = 3;

/// Minimum archive size that benefits from segmented downloads.
const SEGMENTED_DOWNLOAD_MIN_FILE_SIZE: u64 = 128 * 1024 * 1024;

/// Piece sizes are large so big downloads do not create too many requests while
/// still giving multiple workers enough work to do.
const SEGMENTED_DOWNLOAD_SMALL_PIECE_SIZE: u64 = 32 * 1024 * 1024;
const SEGMENTED_DOWNLOAD_LARGE_PIECE_SIZE: u64 = 64 * 1024 * 1024;

/// Cap exponential piece retry backoff to avoid overly long stalls.
const SEGMENTED_DOWNLOAD_MAX_BACKOFF_SECS: u64 = 30;

/// Segmented piece requests should time out quickly enough to recover from slow or stalled
/// requests.
const SEGMENTED_DOWNLOAD_REQUEST_TIMEOUT_SECS: u64 = 120;

/// Paths for one downloaded archive and its `.part` file.
#[derive(Debug, Clone)]
struct DownloadPaths {
    /// User-facing archive file name derived from the URL.
    file_name: String,
    /// Final path for the completed archive file.
    final_path: PathBuf,
    /// Temporary path used while the archive is still downloading.
    part_path: PathBuf,
}

impl DownloadPaths {
    /// Builds the final and partial download paths from the archive URL.
    fn from_url(url: &str, target_dir: &Path) -> Self {
        let file_name = Url::parse(url)
            .ok()
            .and_then(|u| u.path_segments()?.next_back().map(|s| s.to_string()))
            .unwrap_or_else(|| "snapshot.tar".to_string());

        Self {
            final_path: target_dir.join(&file_name),
            part_path: target_dir.join(format!("{file_name}.part")),
            file_name,
        }
    }

    /// Returns the user-facing file name derived from the archive URL.
    fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Returns the final on-disk path for the completed archive.
    fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// Returns the partial download path used while the archive is still in flight.
    fn part_path(&self) -> &Path {
        &self.part_path
    }

    /// Promotes the partial file into the final archive path.
    fn finalize(&self) -> Result<()> {
        fs::rename(&self.part_path, &self.final_path)?;
        Ok(())
    }

    /// Removes only the partial `.part` file for the current archive.
    fn cleanup_partial(&self) {
        let _ = fs::remove_file(&self.part_path);
    }

    /// Removes both final and partial archive files so a fresh attempt can restart cleanly.
    fn cleanup_all(&self) {
        let _ = fs::remove_file(&self.final_path);
        self.cleanup_partial();
    }
}

/// Fetches one archive to disk and chooses sequential or segmented download.
pub(crate) struct ArchiveFetcher {
    /// Remote archive URL.
    url: String,
    /// On-disk paths used for this archive download.
    paths: DownloadPaths,
    /// Shared command-scoped download state.
    session: DownloadSession,
}

impl ArchiveFetcher {
    /// Creates a fetcher for one archive URL under the given target directory.
    pub(crate) fn new(url: impl Into<String>, target_dir: &Path, session: DownloadSession) -> Self {
        let url = url.into();
        let paths = DownloadPaths::from_url(&url, target_dir);
        Self { url, paths, session }
    }

    /// Downloads the archive using the best strategy supported by the remote source.
    pub(crate) fn download(
        &self,
        download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Result<DownloadedArchive> {
        let Some(request_limiter) = self.session.request_limiter() else {
            return self.download_sequential(super::MAX_DOWNLOAD_RETRIES, download_progress)
        };

        let client = BlockingClient::builder().connect_timeout(Duration::from_secs(30)).build()?;
        let probe = self.probe(&client)?;

        match choose_fetch_strategy(probe, request_limiter.max_concurrency()) {
            FetchStrategy::Sequential(reason) => {
                self.log_sequential_fallback(reason, probe.total_size);
                self.download_sequential(super::MAX_DOWNLOAD_RETRIES, download_progress)
            }
            FetchStrategy::Segmented(plan) => {
                self.download_segmented(probe.total_size, plan, download_progress)
            }
        }
    }

    /// Removes any archive files created by this fetcher.
    pub(crate) fn cleanup_downloaded_files(&self) {
        self.paths.cleanup_all();
    }

    /// Probes the remote source for file size and HTTP range support.
    fn probe(&self, client: &BlockingClient) -> Result<RemoteArchiveProbe> {
        let probe = client
            .get(&self.url)
            .header(RANGE, "bytes=0-0")
            .send()
            .and_then(|response| response.error_for_status());

        let (supports_ranges, total_size) = match probe {
            Ok(response) if response.status() == StatusCode::PARTIAL_CONTENT => {
                let total = response
                    .headers()
                    .get("Content-Range")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split('/').next_back())
                    .and_then(|value| value.parse::<u64>().ok());
                (true, total)
            }
            _ => {
                let response = client.head(&self.url).send()?.error_for_status()?;
                (false, response.content_length())
            }
        };

        Ok(RemoteArchiveProbe {
            total_size: total_size.ok_or_else(|| eyre::eyre!("Server did not return file size"))?,
            supports_ranges,
        })
    }

    /// Downloads the archive as a single resumable stream using one request at a time.
    fn download_sequential(
        &self,
        max_download_retries: u32,
        mut download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Result<DownloadedArchive> {
        let quiet = self.quiet();

        if !quiet {
            info!(target: "reth::cli", file = %self.paths.file_name(), "Connecting to download server");
        }

        let client = BlockingClient::builder().timeout(Duration::from_secs(30)).build()?;
        let mut total_size: Option<u64> = None;
        let mut last_error: Option<eyre::Error> = None;

        for attempt in 1..=max_download_retries {
            let existing_size =
                fs::metadata(self.paths.part_path()).map(|meta| meta.len()).unwrap_or(0);

            if let Some(total) = total_size &&
                existing_size >= total
            {
                return self.finalize_download(total)
            }

            if attempt > 1 {
                info!(target: "reth::cli",
                    file = %self.paths.file_name(),
                    "Retry attempt {}/{} - resuming from {} bytes",
                    attempt, max_download_retries, existing_size
                );
            }

            let mut request = client.get(&self.url);
            if existing_size > 0 {
                request = request.header(RANGE, format!("bytes={existing_size}-"));
                if !quiet && attempt == 1 {
                    info!(target: "reth::cli", file = %self.paths.file_name(), "Resuming from {} bytes", existing_size);
                }
            }

            let _request_permit = self
                .session
                .request_limiter()
                .map(|limiter| {
                    limiter.acquire(self.session.progress(), self.session.cancel_token())
                })
                .transpose()?;

            let response = match request.send().and_then(|response| response.error_for_status()) {
                Ok(response) => response,
                Err(error) => {
                    last_error = Some(error.into());
                    if attempt < max_download_retries {
                        info!(target: "reth::cli",
                            file = %self.paths.file_name(),
                            "Download failed, retrying in {RETRY_BACKOFF_SECS}s..."
                        );
                        std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                    }
                    continue;
                }
            };

            let is_partial = response.status() == StatusCode::PARTIAL_CONTENT;
            let size = if is_partial {
                response
                    .headers()
                    .get("Content-Range")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split('/').next_back())
                    .and_then(|value| value.parse().ok())
            } else {
                response.content_length()
            };

            if total_size.is_none() {
                total_size = size;
                if !quiet && let Some(size) = size {
                    info!(target: "reth::cli",
                        file = %self.paths.file_name(),
                        size = %DownloadProgress::format_size(size),
                        "Downloading"
                    );
                }
            }

            let current_total = total_size.ok_or_else(|| {
                eyre::eyre!("Server did not provide Content-Length or Content-Range header")
            })?;

            let file = if is_partial && existing_size > 0 {
                OpenOptions::new()
                    .append(true)
                    .open(self.paths.part_path())
                    .map_err(|error| fs::FsPathError::open(error, self.paths.part_path()))?
            } else {
                fs::create_file(self.paths.part_path())?
            };

            let start_offset = if is_partial { existing_size } else { 0 };
            let mut reader = response;

            let copy_result;
            let flush_result;

            if let Some(progress) = self.session.progress() {
                let mut on_written = |bytes| {
                    if let Some(download_progress) = download_progress.as_deref_mut() {
                        download_progress.record_downloaded(bytes);
                    }
                };
                let mut writer = SharedProgressWriter {
                    inner: BufWriter::new(file),
                    progress: Arc::clone(progress),
                    on_written: Some(&mut on_written),
                };
                copy_result = io::copy(&mut reader, &mut writer);
                flush_result = writer.inner.flush();
            } else {
                let mut progress = DownloadProgress::new(current_total);
                progress.downloaded = start_offset;
                let mut writer = ProgressWriter {
                    inner: BufWriter::new(file),
                    progress,
                    cancel_token: self.session.cancel_token().clone(),
                };
                copy_result = io::copy(&mut reader, &mut writer);
                flush_result = writer.inner.flush();
                println!();
            }

            if let Err(error) = copy_result.and(flush_result) {
                last_error = Some(error.into());
                if attempt < max_download_retries {
                    info!(target: "reth::cli",
                        file = %self.paths.file_name(),
                        "Download interrupted, retrying in {RETRY_BACKOFF_SECS}s..."
                    );
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                }
                continue;
            }

            return self.finalize_download(current_total)
        }

        Err(last_error.unwrap_or_else(|| {
            eyre::eyre!("Download failed after {} attempts", max_download_retries)
        }))
    }

    /// Downloads the archive by splitting it into large range-request pieces.
    fn download_segmented(
        &self,
        total_size: u64,
        plan: SegmentedDownloadPlan,
        download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Result<DownloadedArchive> {
        let request_limiter = self.session.require_request_limiter()?;
        info!(target: "reth::cli",
            total_size = %DownloadProgress::format_size(total_size),
            piece_size = %DownloadProgress::format_size(plan.piece_size),
            pieces = plan.piece_count,
            workers = plan.worker_count,
            max_concurrent_requests = request_limiter.max_concurrency(),
            "Starting queued segmented download"
        );

        SegmentedDownload::new(
            self.url.clone(),
            self.paths.clone(),
            total_size,
            plan,
            self.session.clone(),
            download_progress,
        )
        .run()
    }

    /// Logs why this archive must fall back to the sequential fetch path.
    fn log_sequential_fallback(&self, reason: SequentialDownloadFallback, total_size: u64) {
        match reason {
            SequentialDownloadFallback::NoRangeSupport => {
                info!(target: "reth::cli",
                    file = %self.paths.file_name(),
                    "Server does not support Range requests, falling back to sequential download"
                );
            }
            SequentialDownloadFallback::EmptyFile => {
                info!(target: "reth::cli",
                    file = %self.paths.file_name(),
                    "Remote archive is empty, falling back to sequential download"
                );
            }
            SequentialDownloadFallback::TooSmall => {
                let _ = total_size;
            }
        }
    }

    /// Finalizes the downloaded archive and returns its on-disk location and size.
    fn finalize_download(&self, size: u64) -> Result<DownloadedArchive> {
        self.paths.finalize()?;
        if !self.quiet() {
            info!(target: "reth::cli", file = %self.paths.file_name(), "Download complete");
        }
        Ok(DownloadedArchive { path: self.paths.final_path().to_path_buf(), size })
    }

    /// Returns `true` when this fetch should stay quiet because shared progress is active.
    fn quiet(&self) -> bool {
        self.session.progress().is_some()
    }
}

/// The final path and size of one archive fetched to disk.
#[derive(Debug, Clone)]
pub(crate) struct DownloadedArchive {
    /// Final on-disk path for the downloaded archive.
    pub(crate) path: PathBuf,
    /// Total archive size in bytes.
    pub(crate) size: u64,
}

/// Remote metadata used to choose between sequential and segmented download.
#[derive(Debug, Clone, Copy)]
struct RemoteArchiveProbe {
    /// Total archive size reported by the remote source.
    total_size: u64,
    /// Whether the remote source supports byte-range requests.
    supports_ranges: bool,
}

/// Reasons the fetcher may choose the sequential download path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequentialDownloadFallback {
    /// The remote source does not support byte-range requests.
    NoRangeSupport,
    /// The remote source reported an empty archive.
    EmptyFile,
    /// The archive is too small to benefit from segmented download.
    TooSmall,
}

/// The fetch strategy chosen after probing the remote source.
#[derive(Debug)]
enum FetchStrategy {
    /// Use the single-stream download path.
    Sequential(SequentialDownloadFallback),
    /// Use the segmented download path.
    Segmented(SegmentedDownloadPlan),
}

/// Chooses the fetch strategy from the remote probe and available worker budget.
fn choose_fetch_strategy(probe: RemoteArchiveProbe, max_workers: usize) -> FetchStrategy {
    if !probe.supports_ranges {
        return FetchStrategy::Sequential(SequentialDownloadFallback::NoRangeSupport)
    }

    if probe.total_size == 0 {
        return FetchStrategy::Sequential(SequentialDownloadFallback::EmptyFile)
    }

    plan_segmented_download(probe.total_size, max_workers)
        .map(FetchStrategy::Segmented)
        .unwrap_or(FetchStrategy::Sequential(SequentialDownloadFallback::TooSmall))
}

/// Wrapper that tracks download progress while writing data.
/// Used with [`io::copy`] to display progress during downloads.
struct ProgressWriter<W> {
    /// Wrapped writer receiving downloaded bytes.
    inner: W,
    /// Per-download progress tracker for the legacy path.
    progress: DownloadProgress,
    /// Cancellation token checked between writes.
    cancel_token: CancellationToken,
}

impl<W: Write> Write for ProgressWriter<W> {
    /// Writes bytes, checks cancellation, and updates local download progress.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.cancel_token.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.write(buf)?;
        let _ = self.progress.update(n as u64);
        Ok(n)
    }

    /// Flushes the wrapped writer.
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// One queued byte range for a segmented archive download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadPiece {
    /// Inclusive start byte for this piece.
    start: u64,
    /// Inclusive end byte for this piece.
    end: u64,
}

/// Fixed plan for a segmented archive: piece size, piece count, and worker count.
#[derive(Debug)]
struct SegmentedDownloadPlan {
    /// Bytes assigned to each piece, except possibly the last.
    piece_size: u64,
    /// Number of pieces created for this archive.
    piece_count: usize,
    /// Number of worker threads used for this archive.
    worker_count: usize,
    /// Queue of pieces to download.
    pieces: VecDeque<DownloadPiece>,
}

/// Runs the segmented download workers and piece retries for one archive.
struct SegmentedDownload {
    /// Remote archive URL.
    url: String,
    /// On-disk paths used for this archive download.
    paths: DownloadPaths,
    /// Total archive size in bytes.
    total_size: u64,
    /// Piece and worker plan for this archive.
    plan: SegmentedDownloadPlan,
    /// Shared command-scoped download state.
    session: DownloadSession,
}

/// Shared inputs each segmented download worker needs while draining the piece queue.
#[derive(Clone, Copy)]
struct SegmentedWorkerContext<'a> {
    /// Remote archive URL.
    url: &'a str,
    /// Partial file path where pieces are written.
    part_path: &'a Path,
    /// Shared progress counters for the whole command, when enabled.
    shared: Option<&'a Arc<SharedProgress>>,
    /// Shared cap for in-flight HTTP requests.
    request_limiter: &'a DownloadRequestLimiter,
    /// Cancellation token shared by the whole command.
    cancel_token: &'a CancellationToken,
}

impl SegmentedDownload {
    /// Creates the segmented download state for one archive.
    fn new(
        url: String,
        paths: DownloadPaths,
        total_size: u64,
        plan: SegmentedDownloadPlan,
        session: DownloadSession,
        _download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Self {
        Self { url, paths, total_size, plan, session }
    }

    /// Runs the segmented download to completion or returns the first fatal error.
    fn run(self) -> Result<DownloadedArchive> {
        let Self { url, paths, total_size, plan, session } = self;
        {
            let file = fs::create_file(paths.part_path())?;
            file.set_len(total_size)?;
        }

        let worker_count = plan.worker_count;
        let state = Arc::new(SegmentedDownloadState::new(plan.pieces));
        let terminal_failure = Arc::new(TerminalFailure::default());
        let piece_progress_bytes = Arc::new(AtomicU64::new(0));
        let worker_client = BlockingClient::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(SEGMENTED_DOWNLOAD_REQUEST_TIMEOUT_SECS))
            .build()?;
        let request_limiter = Arc::clone(session.require_request_limiter()?);
        let shared = session.progress();
        let cancel_token = session.cancel_token();
        let url = url.as_str();
        let worker_context = SegmentedWorkerContext {
            url,
            part_path: paths.part_path(),
            shared,
            request_limiter: request_limiter.as_ref(),
            cancel_token,
        };

        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);

            for _ in 0..worker_count {
                let state = Arc::clone(&state);
                let terminal_failure = Arc::clone(&terminal_failure);
                let piece_progress_bytes = Arc::clone(&piece_progress_bytes);
                let client = worker_client.clone();

                handles.push(scope.spawn(move || {
                    Self::worker_loop(
                        &client,
                        worker_context,
                        state,
                        terminal_failure,
                        piece_progress_bytes,
                    );
                }));
            }

            for handle in handles {
                if let Err(payload) = handle.join() {
                    state.note_terminal_failure();
                    terminal_failure.record(eyre::eyre!(
                        "Segmented download worker panicked: {}",
                        panic_payload_message(payload)
                    ));
                }
            }
        });

        if let Some(error) = terminal_failure.take() {
            if let Some(shared) = shared {
                shared.sub_active_download_bytes(piece_progress_bytes.load(Ordering::Relaxed));
            }
            paths.cleanup_partial();
            return Err(error.wrap_err("Parallel download failed"))
        }

        if let Some(shared) = shared {
            shared.sub_active_download_bytes(piece_progress_bytes.load(Ordering::Relaxed));
            shared.record_archive_download_complete(total_size);
        }

        paths.finalize()?;
        info!(target: "reth::cli", file = %paths.file_name(), "Download complete");
        Ok(DownloadedArchive { path: paths.final_path().to_path_buf(), size: total_size })
    }

    /// Runs one worker until there are no pieces left or another worker fails.
    fn worker_loop(
        client: &BlockingClient,
        context: SegmentedWorkerContext<'_>,
        state: Arc<SegmentedDownloadState>,
        terminal_failure: Arc<TerminalFailure>,
        piece_progress_bytes: Arc<AtomicU64>,
    ) {
        let file = match OpenOptions::new().write(true).open(context.part_path) {
            Ok(file) => file,
            Err(error) => {
                state.note_terminal_failure();
                terminal_failure.record(error.into());
                return;
            }
        };

        while let Some(piece) = state.next_piece(context.cancel_token) {
            if let Err(error) = Self::download_piece_with_retries(
                client,
                context.url,
                &file,
                piece,
                context.shared,
                &piece_progress_bytes,
                context.request_limiter,
                context.cancel_token,
            ) {
                state.note_terminal_failure();
                terminal_failure.record(error);
                return;
            }
        }
    }

    /// Downloads one queued piece with per-piece retry/backoff.
    ///
    /// Each attempt acquires a permit from the shared request limit so whole-file and
    /// piece downloads use the same fixed number of HTTP request slots.
    #[expect(clippy::too_many_arguments)]
    fn download_piece_with_retries(
        client: &BlockingClient,
        url: &str,
        file: &std::fs::File,
        piece: DownloadPiece,
        shared: Option<&Arc<SharedProgress>>,
        piece_progress_bytes: &AtomicU64,
        request_limiter: &DownloadRequestLimiter,
        cancel_token: &CancellationToken,
    ) -> Result<()> {
        for attempt in 1..=SEGMENT_RETRY_ATTEMPTS {
            if cancel_token.is_cancelled() {
                return Err(eyre::eyre!("Download cancelled"))
            }

            let _request_permit = request_limiter.acquire(shared, cancel_token)?;
            match Self::download_piece_once(
                client,
                url,
                file,
                piece,
                shared,
                piece_progress_bytes,
                cancel_token,
            ) {
                Ok(()) => return Ok(()),
                Err(PieceAttemptFailure::Retryable { error: _, throttled })
                    if attempt < SEGMENT_RETRY_ATTEMPTS =>
                {
                    std::thread::sleep(piece_retry_backoff(attempt, throttled));
                }
                Err(PieceAttemptFailure::Retryable { error, .. }) => return Err(error),
                Err(PieceAttemptFailure::Terminal(error)) => return Err(error),
            }
        }

        Err(eyre::eyre!("Piece download failed after {SEGMENT_RETRY_ATTEMPTS} attempts"))
    }

    /// Downloads one queued piece once.
    fn download_piece_once(
        client: &BlockingClient,
        url: &str,
        file: &std::fs::File,
        piece: DownloadPiece,
        shared: Option<&Arc<SharedProgress>>,
        piece_progress_bytes: &AtomicU64,
        cancel_token: &CancellationToken,
    ) -> std::result::Result<(), PieceAttemptFailure> {
        use std::os::unix::fs::FileExt;

        let expected_len = piece.end - piece.start + 1;

        let response = match client
            .get(url)
            .header(RANGE, format!("bytes={}-{}", piece.start, piece.end))
            .send()
        {
            Ok(response) if response.status() == StatusCode::PARTIAL_CONTENT => response,
            Ok(response) if should_retry_piece_status(response.status()) => {
                return Err(PieceAttemptFailure::Retryable {
                    error: eyre::eyre!(
                        "Server returned {} for piece {}-{}",
                        response.status(),
                        piece.start,
                        piece.end
                    ),
                    throttled: is_throttle_piece_status(response.status()),
                });
            }
            Ok(response) => {
                return Err(PieceAttemptFailure::Terminal(eyre::eyre!(
                    "Server returned {} instead of 206 for Range request",
                    response.status()
                )));
            }
            Err(error) => {
                return Err(PieceAttemptFailure::Retryable {
                    throttled: is_throttle_piece_error(&error),
                    error: error.into(),
                });
            }
        };

        let mut buf = [0u8; 64 * 1024];
        let mut reader = response.take(expected_len);
        let mut offset = piece.start;

        loop {
            if cancel_token.is_cancelled() {
                return Err(PieceAttemptFailure::Terminal(eyre::eyre!("Download cancelled")));
            }

            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    file.write_all_at(&buf[..n], offset)
                        .map_err(|error| PieceAttemptFailure::Terminal(error.into()))?;
                    offset += n as u64;
                    if let Some(progress) = shared {
                        progress.record_session_fetched_bytes(n as u64);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(PieceAttemptFailure::Retryable {
                        throttled: error.kind() == io::ErrorKind::TimedOut,
                        error: error.into(),
                    });
                }
            }
        }

        let downloaded_len = offset - piece.start;
        if downloaded_len == expected_len {
            if let Some(progress) = shared {
                progress.add_active_download_bytes(expected_len);
            }
            piece_progress_bytes.fetch_add(expected_len, Ordering::Relaxed);
            return Ok(())
        }

        Err(PieceAttemptFailure::Retryable {
            error: eyre::eyre!(
                "Piece {}-{} ended early: expected {} bytes, downloaded {}",
                piece.start,
                piece.end,
                expected_len,
                downloaded_len
            ),
            throttled: false,
        })
    }
}

/// Shared queue state for one segmented archive download.
///
/// Workers pull pieces until the queue is empty or one worker fails the whole attempt.
struct SegmentedDownloadState {
    /// Remaining pieces waiting to be downloaded.
    pieces: Mutex<VecDeque<DownloadPiece>>,
    /// Set once a worker hits a fatal error.
    failed: AtomicBool,
}

impl SegmentedDownloadState {
    /// Creates the shared queue state for one segmented archive attempt.
    fn new(pieces: VecDeque<DownloadPiece>) -> Self {
        Self { pieces: Mutex::new(pieces), failed: AtomicBool::new(false) }
    }

    /// Returns the next piece unless cancellation or a fatal error stopped the attempt.
    fn next_piece(&self, cancel_token: &CancellationToken) -> Option<DownloadPiece> {
        if cancel_token.is_cancelled() || self.failed.load(Ordering::Relaxed) {
            return None;
        }

        self.pieces.lock().unwrap().pop_front()
    }

    /// Marks the entire segmented attempt as failed so workers stop taking more pieces.
    fn note_terminal_failure(&self) {
        self.failed.store(true, Ordering::Relaxed);
    }
}

/// Stores the first fatal error seen across segmented download workers.
#[derive(Default)]
struct TerminalFailure {
    /// First fatal worker error, if any.
    error: Mutex<Option<eyre::Error>>,
}

impl TerminalFailure {
    /// Stores the first fatal error and ignores later ones from other workers.
    fn record(&self, error: eyre::Error) {
        let mut slot = self.error.lock().unwrap();
        if slot.is_none() {
            *slot = Some(error);
        }
    }

    /// Returns the stored fatal error after worker execution finishes.
    fn take(&self) -> Option<eyre::Error> {
        self.error.lock().unwrap().take()
    }
}

/// Splits an archive into contiguous byte ranges for segmented download.
fn build_download_pieces(total_size: u64, piece_size: u64) -> VecDeque<DownloadPiece> {
    let mut pieces = VecDeque::new();
    let mut start = 0;

    while start < total_size {
        let end = (start + piece_size).min(total_size) - 1;
        pieces.push_back(DownloadPiece { start, end });
        start = end + 1;
    }

    pieces
}

/// Chooses the fixed piece size for a large archive.
///
/// Smaller large files use 32 MiB pieces so there are enough pieces for several workers.
/// Very large files use 64 MiB pieces to keep the request count down.
fn segmented_piece_size(total_size: u64) -> u64 {
    if total_size < 2 * 1024 * 1024 * 1024 {
        SEGMENTED_DOWNLOAD_SMALL_PIECE_SIZE
    } else {
        SEGMENTED_DOWNLOAD_LARGE_PIECE_SIZE
    }
}

/// Builds the segmented download plan for one archive.
///
/// Small files stay single-stream. Larger files are split into fixed pieces and
/// can use up to the shared request limit.
fn plan_segmented_download(total_size: u64, max_workers: usize) -> Option<SegmentedDownloadPlan> {
    if max_workers == 0 || total_size < SEGMENTED_DOWNLOAD_MIN_FILE_SIZE {
        return None;
    }

    let piece_size = segmented_piece_size(total_size);
    if total_size <= piece_size {
        return None;
    }

    let pieces = build_download_pieces(total_size, piece_size);
    let piece_count = pieces.len();
    let worker_count = max_workers.min(piece_count).max(1);

    Some(SegmentedDownloadPlan { piece_size, piece_count, worker_count, pieces })
}

/// Returns the retry backoff for one piece attempt.
fn piece_retry_backoff(attempt: u32, throttled: bool) -> Duration {
    let base = if throttled { 2 } else { RETRY_BACKOFF_SECS };
    let multiplier = 1u64 << attempt.saturating_sub(1).min(3);
    Duration::from_secs(base.saturating_mul(multiplier).min(SEGMENTED_DOWNLOAD_MAX_BACKOFF_SECS))
}

/// Returns whether an HTTP status should retry the current piece.
fn is_retryable_piece_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT |
            StatusCode::TOO_MANY_REQUESTS |
            StatusCode::INTERNAL_SERVER_ERROR |
            StatusCode::BAD_GATEWAY |
            StatusCode::SERVICE_UNAVAILABLE |
            StatusCode::GATEWAY_TIMEOUT
    )
}

/// Returns whether a piece request should retry after the given status.
fn should_retry_piece_status(status: StatusCode) -> bool {
    status == StatusCode::OK || is_retryable_piece_status(status)
}

/// Returns whether an HTTP status looks like throttling or timeout.
fn is_throttle_piece_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT |
            StatusCode::TOO_MANY_REQUESTS |
            StatusCode::SERVICE_UNAVAILABLE |
            StatusCode::GATEWAY_TIMEOUT
    )
}

/// Returns whether a reqwest error looks like throttling or timeout.
fn is_throttle_piece_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || matches!(error.status(), Some(status) if is_throttle_piece_status(status))
}

/// The result of one piece download attempt.
enum PieceAttemptFailure {
    /// The piece can be retried.
    Retryable { error: eyre::Error, throttled: bool },
    /// The piece failed in a way that should stop the archive.
    Terminal(eyre::Error),
}

/// Converts a thread panic payload into a readable message.
fn panic_payload_message(payload: Box<dyn Any + Send + 'static>) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn segmented_plan_skips_small_files() {
        assert!(plan_segmented_download(SEGMENTED_DOWNLOAD_MIN_FILE_SIZE - 1, 16).is_none());
    }

    #[test]
    fn segmented_plan_uses_large_pieces_and_adaptive_workers() {
        let total_size = 512 * 1024 * 1024;
        let plan = plan_segmented_download(total_size, 32).unwrap();

        assert_eq!(plan.piece_size, SEGMENTED_DOWNLOAD_SMALL_PIECE_SIZE);
        assert_eq!(plan.piece_count, 16);
        assert_eq!(plan.worker_count, 16);
    }

    #[test]
    fn build_download_pieces_covers_entire_file() {
        let pieces = build_download_pieces(10, 4).into_iter().collect::<Vec<_>>();

        assert_eq!(
            pieces,
            vec![
                DownloadPiece { start: 0, end: 3 },
                DownloadPiece { start: 4, end: 7 },
                DownloadPiece { start: 8, end: 9 },
            ]
        );
    }

    #[test]
    fn piece_status_retry_policy_retries_200_ok() {
        assert!(should_retry_piece_status(StatusCode::OK));
        assert!(should_retry_piece_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(!should_retry_piece_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn choose_fetch_strategy_uses_segmented_when_ranges_are_supported() {
        let strategy = choose_fetch_strategy(
            RemoteArchiveProbe { total_size: 512 * 1024 * 1024, supports_ranges: true },
            16,
        );

        assert!(matches!(strategy, FetchStrategy::Segmented(_)));
    }

    #[test]
    fn choose_fetch_strategy_falls_back_without_ranges() {
        let strategy = choose_fetch_strategy(
            RemoteArchiveProbe { total_size: 512 * 1024 * 1024, supports_ranges: false },
            16,
        );

        assert!(matches!(
            strategy,
            FetchStrategy::Sequential(SequentialDownloadFallback::NoRangeSupport)
        ));
    }
}
