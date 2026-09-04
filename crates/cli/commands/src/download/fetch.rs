use super::{
    progress::{
        ArchiveDownloadProgress, DownloadProgress, DownloadRequestLimiter, SharedProgress,
        SharedProgressWriter,
    },
    session::DownloadSession,
    RETRY_BACKOFF_SECS,
};
use eyre::Result;
use reqwest::{
    blocking::Client as BlockingClient,
    header::{CONTENT_RANGE, RANGE},
    StatusCode,
};
use reth_cli_util::cancellation::CancellationToken;
use reth_fs_util as fs;
use serde::{Deserialize, Serialize};
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

/// Version of the persisted segmented resume-state schema.
const SEGMENTED_RESUME_STATE_VERSION: u8 = 1;

/// Paths for one downloaded archive and its `.part` file.
#[derive(Debug, Clone)]
struct DownloadPaths {
    /// User-facing archive file name derived from the URL.
    file_name: String,
    /// Final path for the completed archive file.
    final_path: PathBuf,
    /// Temporary path used while the archive is still downloading.
    part_path: PathBuf,
    /// Persisted completion state for segmented downloads.
    resume_state_path: PathBuf,
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
            resume_state_path: target_dir.join(format!("{file_name}.part.json")),
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

    /// Returns the sidecar used to persist completed download pieces.
    fn resume_state_path(&self) -> &Path {
        &self.resume_state_path
    }

    /// Promotes the partial file into the final archive path.
    fn finalize(&self) -> Result<()> {
        fs::rename(&self.part_path, &self.final_path)?;
        let _ = fs::remove_file(&self.resume_state_path);
        Ok(())
    }

    /// Removes only the partial `.part` file for the current archive.
    fn cleanup_partial(&self) {
        let _ = fs::remove_file(&self.part_path);
        let _ = fs::remove_file(&self.resume_state_path);
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
    /// Optional manifest checksum used to identify resumable state.
    checksum: Option<String>,
}

impl ArchiveFetcher {
    /// Creates a fetcher for one archive URL under the given target directory.
    pub(crate) fn new(
        url: impl Into<String>,
        target_dir: &Path,
        session: DownloadSession,
        checksum: Option<String>,
    ) -> Self {
        let url = url.into();
        let paths = DownloadPaths::from_url(&url, target_dir);
        Self { url, paths, session, checksum }
    }

    /// Downloads the archive using the best strategy supported by the remote source.
    pub(crate) fn download(
        &self,
        download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Result<DownloadedArchive> {
        if let Some(path) = archive_file_url_path(&self.url)? {
            let size = fs::metadata(&path)?.len();
            if !self.quiet() {
                info!(target: "reth::cli",
                    file = %path.display(),
                    size = %DownloadProgress::format_size(size),
                    "Using local archive"
                );
            }
            return Ok(DownloadedArchive { path, size })
        }

        let Some(request_limiter) = self.session.request_limiter() else {
            return self.download_sequential(super::MAX_DOWNLOAD_RETRIES, download_progress)
        };

        let client = BlockingClient::builder().connect_timeout(Duration::from_secs(30)).build()?;
        let probe = self.probe(&client)?;

        match choose_fetch_strategy(probe, request_limiter.max_concurrency()) {
            FetchStrategy::Sequential(reason) => {
                if reason == SequentialDownloadFallback::NoRangeSupport &&
                    SegmentedResumeStateStore::has_completed_pieces(
                        &self.paths,
                        &self.url,
                        self.checksum.as_deref(),
                        probe.total_size,
                        request_limiter.max_concurrency(),
                    )
                {
                    eyre::bail!(
                        "Server did not accept the Range request required to resume the partial download"
                    );
                }
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
                    .get(CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_content_range)
                    .and_then(|(start, end, total)| (start == 0 && end == 0).then_some(total));
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

        if fs::metadata(self.paths.resume_state_path()).is_ok() {
            self.paths.cleanup_partial();
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
                let (start, _, total) = response
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(parse_content_range)
                    .ok_or_else(|| eyre::eyre!("Server returned invalid Content-Range"))?;
                eyre::ensure!(
                    start == existing_size,
                    "Server returned mismatched Content-Range for resume offset {existing_size}"
                );
                Some(total)
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
            self.checksum.clone(),
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

/// Resolves a `file://` archive URL to its local path.
fn archive_file_url_path(url: &str) -> Result<Option<PathBuf>> {
    let Ok(parsed) = Url::parse(url) else { return Ok(None) };
    if parsed.scheme() != "file" {
        return Ok(None)
    }

    parsed
        .to_file_path()
        .map(Some)
        .map_err(|_| eyre::eyre!("Invalid file:// archive URL path: {url}"))
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
    /// Stable index in the segmented download plan.
    index: usize,
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

/// Durable completion bitmap for one segmented archive download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SegmentedResumeState {
    /// Sidecar schema version.
    version: u8,
    /// Source URL whose bytes are represented by the partial file.
    url: String,
    /// Optional manifest checksum used to distinguish archive revisions.
    checksum: Option<String>,
    /// Expected compressed archive size.
    total_size: u64,
    /// Piece size used to build the download plan.
    piece_size: u64,
    /// Completion flag for every piece in plan order.
    completed: Vec<bool>,
}

impl SegmentedResumeState {
    /// Creates empty resume state for a new segmented download.
    fn new(
        url: &str,
        checksum: Option<&str>,
        total_size: u64,
        piece_size: u64,
        piece_count: usize,
    ) -> Self {
        Self {
            version: SEGMENTED_RESUME_STATE_VERSION,
            url: url.to_string(),
            checksum: checksum.map(ToOwned::to_owned),
            total_size,
            piece_size,
            completed: vec![false; piece_count],
        }
    }

    /// Returns whether this state belongs to the current archive and piece plan.
    fn matches(&self, expected: &Self) -> bool {
        self.version == expected.version &&
            self.url == expected.url &&
            self.checksum == expected.checksum &&
            self.total_size == expected.total_size &&
            self.piece_size == expected.piece_size &&
            self.completed.len() == expected.completed.len()
    }
}

/// Thread-safe owner of a segmented resume sidecar.
struct SegmentedResumeStateStore {
    /// Sidecar path updated after each completed piece.
    path: PathBuf,
    /// Current in-memory completion state.
    state: Mutex<SegmentedResumeState>,
}

impl SegmentedResumeStateStore {
    /// Returns whether matching state contains completed pieces worth preserving.
    fn has_completed_pieces(
        paths: &DownloadPaths,
        url: &str,
        checksum: Option<&str>,
        total_size: u64,
        max_workers: usize,
    ) -> bool {
        let Some(plan) = plan_segmented_download(total_size, max_workers) else { return false };
        let expected =
            SegmentedResumeState::new(url, checksum, total_size, plan.piece_size, plan.piece_count);
        let Ok(state) = fs::read_json_file::<SegmentedResumeState>(paths.resume_state_path())
        else {
            return false
        };

        fs::metadata(paths.part_path()).is_ok_and(|metadata| metadata.len() == total_size) &&
            state.matches(&expected) &&
            state.completed.iter().any(|completed| *completed)
    }

    /// Loads matching resume state or safely starts a new partial download.
    fn load_or_create(
        paths: &DownloadPaths,
        url: &str,
        checksum: Option<&str>,
        total_size: u64,
        plan: &SegmentedDownloadPlan,
    ) -> Result<(Arc<Self>, VecDeque<DownloadPiece>, u64)> {
        let expected =
            SegmentedResumeState::new(url, checksum, total_size, plan.piece_size, plan.piece_count);
        let existing = fs::read_json_file::<SegmentedResumeState>(paths.resume_state_path()).ok();
        let part_matches =
            fs::metadata(paths.part_path()).is_ok_and(|metadata| metadata.len() == total_size);
        let state = match existing {
            Some(state) if part_matches && state.matches(&expected) => state,
            _ => {
                paths.cleanup_partial();
                let file = fs::create_file(paths.part_path())?;
                file.set_len(total_size)?;
                Self::persist(paths.resume_state_path(), &expected)?;
                expected
            }
        };

        let mut pending = VecDeque::new();
        let mut completed_bytes = 0;
        for piece in &plan.pieces {
            if state.completed[piece.index] {
                completed_bytes += piece.end - piece.start + 1;
            } else {
                pending.push_back(*piece);
            }
        }

        let store = Arc::new(Self {
            path: paths.resume_state_path().to_path_buf(),
            state: Mutex::new(state),
        });
        Ok((store, pending, completed_bytes))
    }

    /// Marks one fully written piece complete and atomically persists the bitmap.
    fn mark_complete(&self, piece: DownloadPiece) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        state.completed[piece.index] = true;
        Self::persist(&self.path, &state)
    }

    /// Atomically writes the sidecar so interrupted updates keep the previous bitmap.
    fn persist(path: &Path, state: &SegmentedResumeState) -> Result<()> {
        fs::atomic_write_file(path, |file| serde_json::to_writer(file, state))?;
        Ok(())
    }
}

/// Runs the segmented download workers and piece retries for one archive.
struct SegmentedDownload {
    /// Remote archive URL.
    url: String,
    /// On-disk paths used for this archive download.
    paths: DownloadPaths,
    /// Optional manifest checksum used to identify resumable state.
    checksum: Option<String>,
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
    /// Expected compressed archive size.
    total_size: u64,
    /// Partial file path where pieces are written.
    part_path: &'a Path,
    /// Shared progress counters for the whole command, when enabled.
    shared: Option<&'a Arc<SharedProgress>>,
    /// Shared cap for in-flight HTTP requests.
    request_limiter: &'a DownloadRequestLimiter,
    /// Cancellation token shared by the whole command.
    cancel_token: &'a CancellationToken,
    /// Persisted completion state updated after each successful piece.
    resume_state: &'a SegmentedResumeStateStore,
}

impl SegmentedDownload {
    /// Creates the segmented download state for one archive.
    fn new(
        url: String,
        paths: DownloadPaths,
        checksum: Option<String>,
        total_size: u64,
        plan: SegmentedDownloadPlan,
        session: DownloadSession,
        _download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
    ) -> Self {
        Self { url, paths, checksum, total_size, plan, session }
    }

    /// Runs the segmented download to completion or returns the first fatal error.
    fn run(self) -> Result<DownloadedArchive> {
        let Self { url, paths, checksum, total_size, plan, session } = self;
        let (resume_state, pending_pieces, resumed_bytes) =
            SegmentedResumeStateStore::load_or_create(
                &paths,
                &url,
                checksum.as_deref(),
                total_size,
                &plan,
            )?;

        let worker_count = plan.worker_count;
        let state = Arc::new(SegmentedDownloadState::new(pending_pieces));
        let terminal_failure = Arc::new(TerminalFailure::default());
        let piece_progress_bytes = Arc::new(AtomicU64::new(resumed_bytes));
        let worker_client = BlockingClient::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(SEGMENTED_DOWNLOAD_REQUEST_TIMEOUT_SECS))
            .build()?;
        let request_limiter = Arc::clone(session.require_request_limiter()?);
        let shared = session.progress();
        if resumed_bytes > 0 {
            if let Some(shared) = shared {
                shared.add_active_download_bytes(resumed_bytes);
            }
            info!(target: "reth::cli",
                file = %paths.file_name(),
                resumed = %DownloadProgress::format_size(resumed_bytes),
                "Resuming segmented download"
            );
        }
        let cancel_token = session.cancel_token();
        let url = url.as_str();
        let worker_context = SegmentedWorkerContext {
            url,
            total_size,
            part_path: paths.part_path(),
            shared,
            request_limiter: request_limiter.as_ref(),
            cancel_token,
            resume_state: resume_state.as_ref(),
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

        let downloaded_bytes = piece_progress_bytes.load(Ordering::Relaxed);
        if let Some(error) = terminal_failure.take() {
            if let Some(shared) = shared {
                shared.sub_active_download_bytes(downloaded_bytes);
            }
            return Err(error.wrap_err("Parallel download failed"))
        }

        if cancel_token.is_cancelled() {
            if let Some(shared) = shared {
                shared.sub_active_download_bytes(downloaded_bytes);
            }
            eyre::bail!("Parallel download cancelled");
        }

        if downloaded_bytes != total_size {
            if let Some(shared) = shared {
                shared.sub_active_download_bytes(downloaded_bytes);
            }
            paths.cleanup_partial();
            eyre::bail!("Parallel download did not complete");
        }

        if let Some(shared) = shared {
            shared.sub_active_download_bytes(downloaded_bytes);
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
                context.total_size,
                &file,
                piece,
                context.shared,
                &piece_progress_bytes,
                context.request_limiter,
                context.cancel_token,
                context.resume_state,
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
        total_size: u64,
        file: &std::fs::File,
        piece: DownloadPiece,
        shared: Option<&Arc<SharedProgress>>,
        piece_progress_bytes: &AtomicU64,
        request_limiter: &DownloadRequestLimiter,
        cancel_token: &CancellationToken,
        resume_state: &SegmentedResumeStateStore,
    ) -> Result<()> {
        for attempt in 1..=SEGMENT_RETRY_ATTEMPTS {
            if cancel_token.is_cancelled() {
                return Err(eyre::eyre!("Download cancelled"))
            }

            let _request_permit = request_limiter.acquire(shared, cancel_token)?;
            match Self::download_piece_once(
                client,
                url,
                total_size,
                file,
                piece,
                shared,
                piece_progress_bytes,
                cancel_token,
            ) {
                Ok(()) => {
                    file.sync_data()?;
                    resume_state.mark_complete(piece)?;
                    return Ok(())
                }
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
    #[expect(clippy::too_many_arguments)]
    fn download_piece_once(
        client: &BlockingClient,
        url: &str,
        total_size: u64,
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

        let returned_range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range);
        if returned_range != Some((piece.start, piece.end, total_size)) {
            return Err(PieceAttemptFailure::Terminal(eyre::eyre!(
                "Server returned invalid Content-Range for piece {}-{}",
                piece.start,
                piece.end
            )));
        }

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
    let mut index = 0;

    while start < total_size {
        let end = (start + piece_size).min(total_size) - 1;
        pieces.push_back(DownloadPiece { index, start, end });
        start = end + 1;
        index += 1;
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

/// Parses an HTTP `Content-Range` value into its inclusive range and total size.
fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end && end < total).then_some((start, end, total))
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
    use reth_cli_util::cancellation::CancellationToken;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        thread::JoinHandle,
    };

    fn read_request(stream: &TcpStream) -> String {
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream);
        let mut request = String::new();
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0, "incomplete HTTP headers");
            if line == "\r\n" {
                break;
            }
            request.push_str(&line);
        }
        request.to_ascii_lowercase()
    }

    fn spawn_range_server(
        expected_range: &str,
        start: u64,
        end: u64,
        total: u64,
        body: &'static [u8],
    ) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let expected_range = expected_range.to_ascii_lowercase();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&stream);
            assert!(request.contains(&format!("range: {expected_range}")));
            write!(
                stream,
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (format!("http://{address}/state.tar.zst"), handle)
    }

    #[test]
    fn sequential_resume_rejects_mismatched_content_range() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = spawn_range_server("bytes=4-", 0, 3, 8, b"efgh");
        let part = dir.path().join("state.tar.zst.part");
        fs::write(&part, b"abcd").unwrap();
        let session = DownloadSession::new(None, None, CancellationToken::new());
        let fetcher = ArchiveFetcher::new(url, dir.path(), session, None);
        let error = fetcher.download_sequential(1, None).unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("mismatched Content-Range"));
        assert_eq!(fs::read(&part).unwrap(), b"abcd");
        assert!(!dir.path().join("state.tar.zst").exists());
    }

    #[test]
    fn stale_segmented_state_is_discarded() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DownloadPaths::from_url("https://example.com/state.tar.zst", dir.path());
        let pieces = build_download_pieces(8, 4);
        let plan = SegmentedDownloadPlan { piece_size: 4, piece_count: 2, worker_count: 1, pieces };
        for truncated in [false, true] {
            let mut state = SegmentedResumeState::new(
                "https://example.com/state.tar.zst",
                Some("old"),
                8,
                4,
                2,
            );
            state.completed[0] = true;
            SegmentedResumeStateStore::persist(paths.resume_state_path(), &state).unwrap();
            fs::write(
                paths.part_path(),
                if truncated { b"abcd".as_slice() } else { b"abcdefgh".as_slice() },
            )
            .unwrap();
            let checksum = if truncated { "old" } else { "new" };
            let (_, pending, completed) = SegmentedResumeStateStore::load_or_create(
                &paths,
                &state.url,
                Some(checksum),
                8,
                &plan,
            )
            .unwrap();
            assert_eq!(completed, 0);
            assert_eq!(pending.len(), 2);
            assert_eq!(fs::read(paths.part_path()).unwrap(), [0; 8]);
        }
    }

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
                DownloadPiece { index: 0, start: 0, end: 3 },
                DownloadPiece { index: 1, start: 4, end: 7 },
                DownloadPiece { index: 2, start: 8, end: 9 },
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

    #[test]
    fn archive_fetcher_uses_file_url_archive_directly() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("state.tar.zst");
        {
            let mut archive = std::fs::File::create(&archive_path).unwrap();
            archive.write_all(b"local archive bytes").unwrap();
        }

        let cache_dir = dir.path().join("cache");
        std::fs::create_dir(&cache_dir).unwrap();
        let url = Url::from_file_path(&archive_path).unwrap().to_string();
        let session = DownloadSession::new(None, None, CancellationToken::new());
        let fetcher = ArchiveFetcher::new(url, &cache_dir, session, None);

        let downloaded = fetcher.download(None).unwrap();

        assert_eq!(downloaded.path, archive_path);
        assert_eq!(downloaded.size, b"local archive bytes".len() as u64);
        assert!(!cache_dir.join("state.tar.zst").exists());
        assert!(!cache_dir.join("state.tar.zst.part").exists());
    }

    #[test]
    fn segmented_download_requests_only_missing_pieces_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/state.tar.zst", listener.local_addr().unwrap());
        let first_cancel_token = CancellationToken::new();
        let server_cancel_token = first_cancel_token.clone();
        let server = std::thread::spawn(move || {
            for (index, (expected_range, start, end, body)) in [
                ("bytes=0-3", 0, 3, b"abcd"),
                ("bytes=4-7", 4, 7, b"efgh"),
                ("bytes=4-7", 4, 7, b"efgh"),
            ]
            .into_iter()
            .enumerate()
            {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&stream);
                assert!(request.contains(&format!("range: {expected_range}")));
                if index == 1 {
                    server_cancel_token.cancel();
                }
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/8\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                let _ = stream.write_all(body);
            }
        });
        let paths = DownloadPaths::from_url(&url, dir.path());
        let pieces = build_download_pieces(8, 4);
        let plan = SegmentedDownloadPlan {
            piece_size: 4,
            piece_count: pieces.len(),
            worker_count: 1,
            pieces,
        };
        let progress = SharedProgress::new(8, 0, 1, first_cancel_token.clone());
        let session = DownloadSession::new(
            Some(Arc::clone(&progress)),
            Some(DownloadRequestLimiter::new(1)),
            first_cancel_token,
        );
        let download = SegmentedDownload::new(
            url.clone(),
            paths.clone(),
            Some("checksum".to_string()),
            8,
            plan,
            session,
            None,
        );

        assert!(download.run().is_err());
        let resume_state =
            fs::read_json_file::<SegmentedResumeState>(paths.resume_state_path()).unwrap();
        assert_eq!(resume_state.completed, [true, false]);

        let pieces = build_download_pieces(8, 4);
        let plan = SegmentedDownloadPlan {
            piece_size: 4,
            piece_count: pieces.len(),
            worker_count: 1,
            pieces,
        };
        let cancel_token = CancellationToken::new();
        let resumed_progress = SharedProgress::new(8, 0, 1, cancel_token.clone());
        let session = DownloadSession::new(
            Some(Arc::clone(&resumed_progress)),
            Some(DownloadRequestLimiter::new(1)),
            cancel_token,
        );
        let download = SegmentedDownload::new(
            url,
            paths,
            Some("checksum".to_string()),
            8,
            plan,
            session,
            None,
        );
        let downloaded = download.run().unwrap();
        server.join().unwrap();

        assert_eq!(std::fs::read(downloaded.path).unwrap(), b"abcdefgh");
        assert_eq!(resumed_progress.active_download_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(resumed_progress.session_fetched_bytes.load(Ordering::Relaxed), 4);
        assert!(!dir.path().join("state.tar.zst.part.json").exists());
    }

    #[test]
    fn segmented_download_does_not_trust_partial_file_without_matching_state() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://example.com/state.tar.zst";
        let paths = DownloadPaths::from_url(url, dir.path());
        std::fs::write(paths.part_path(), b"stale-bytes").unwrap();
        let pieces = build_download_pieces(11, 4);
        let plan = SegmentedDownloadPlan {
            piece_size: 4,
            piece_count: pieces.len(),
            worker_count: 1,
            pieces,
        };

        let (_, pending, completed_bytes) =
            SegmentedResumeStateStore::load_or_create(&paths, url, Some("checksum"), 11, &plan)
                .unwrap();

        assert_eq!(pending.len(), 3);
        assert_eq!(completed_bytes, 0);
        assert_eq!(std::fs::read(paths.part_path()).unwrap(), [0; 11]);
    }

    #[test]
    fn segmented_download_preserves_matching_completed_pieces_on_range_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://example.com/state.tar.zst";
        let paths = DownloadPaths::from_url(url, dir.path());
        let total_size = SEGMENTED_DOWNLOAD_MIN_FILE_SIZE;
        let file = std::fs::File::create(paths.part_path()).unwrap();
        file.set_len(total_size).unwrap();
        let plan = plan_segmented_download(total_size, 4).unwrap();
        let mut state = SegmentedResumeState::new(
            url,
            Some("checksum"),
            total_size,
            plan.piece_size,
            plan.piece_count,
        );
        state.completed[0] = true;
        SegmentedResumeStateStore::persist(paths.resume_state_path(), &state).unwrap();

        assert!(SegmentedResumeStateStore::has_completed_pieces(
            &paths,
            url,
            Some("checksum"),
            total_size,
            4,
        ));
        assert!(!SegmentedResumeStateStore::has_completed_pieces(
            &paths,
            url,
            Some("different-checksum"),
            total_size,
            4,
        ));
    }

    #[test]
    fn segmented_download_rejects_mismatched_content_range() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = spawn_range_server("bytes=0-3", 1, 4, 4, b"abcd");
        let paths = DownloadPaths::from_url(&url, dir.path());
        let pieces = build_download_pieces(4, 4);
        let plan = SegmentedDownloadPlan {
            piece_size: 4,
            piece_count: pieces.len(),
            worker_count: 1,
            pieces,
        };
        let cancel_token = CancellationToken::new();
        let session =
            DownloadSession::new(None, Some(DownloadRequestLimiter::new(1)), cancel_token);
        let download = SegmentedDownload::new(url, paths.clone(), None, 4, plan, session, None);

        assert!(download.run().is_err());
        server.join().unwrap();
        let resume_state =
            fs::read_json_file::<SegmentedResumeState>(paths.resume_state_path()).unwrap();
        assert_eq!(resume_state.completed, [false]);
    }

    #[test]
    fn cancelled_segmented_download_preserves_resumable_state() {
        let dir = tempfile::tempdir().unwrap();
        let total_size = 8;
        let pieces = build_download_pieces(total_size, 4);
        let plan = SegmentedDownloadPlan {
            piece_size: 4,
            piece_count: pieces.len(),
            worker_count: 1,
            pieces,
        };
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let progress = SharedProgress::new(total_size, 0, 1, cancel_token.clone());
        let session = DownloadSession::new(
            Some(progress),
            Some(DownloadRequestLimiter::new(1)),
            cancel_token,
        );
        let url = "http://127.0.0.1:1/state.tar.zst";
        let paths = DownloadPaths::from_url(url, dir.path());
        let download =
            SegmentedDownload::new(url.to_string(), paths, None, total_size, plan, session, None);

        assert!(download.run().is_err());
        assert!(!dir.path().join("state.tar.zst").exists());
        assert!(dir.path().join("state.tar.zst.part").exists());
        assert!(dir.path().join("state.tar.zst.part.json").exists());
    }

    #[test]
    fn parses_content_range() {
        assert_eq!(parse_content_range("bytes 4-7/8"), Some((4, 7, 8)));
        assert_eq!(parse_content_range("bytes 4-7/*"), None);
        assert_eq!(parse_content_range("4-7/8"), None);
    }
}
