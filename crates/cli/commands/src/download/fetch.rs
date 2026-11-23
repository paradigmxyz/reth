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
    collections::{BTreeMap, VecDeque},
    fs::{File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tracing::{info, warn};
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

/// Bytes a piece download buffers before each write to the shared `.part` file.
///
/// Filesystems such as ext4 take an exclusive per-inode lock for every buffered write, so many
/// pieces writing small chunks into one file contend on that lock instead of the disk.
const SEGMENTED_DOWNLOAD_WRITE_BUFFER_SIZE: usize = 8 * 1024 * 1024;

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
    /// Archive size declared by the manifest. `None` trusts the size the server reports.
    expected_size: Option<u64>,
}

impl ArchiveFetcher {
    /// Creates a fetcher for one archive URL under the given target directory.
    ///
    /// When `expected_size` is set, the download fails if the server reports any other size.
    pub(crate) fn new(
        url: impl Into<String>,
        target_dir: &Path,
        session: DownloadSession,
        expected_size: Option<u64>,
    ) -> Self {
        let url = url.into();
        let paths = DownloadPaths::from_url(&url, target_dir);
        Self { url, paths, session, expected_size }
    }

    /// Downloads the archive using the best strategy supported by the remote source.
    ///
    /// When `availability` is set, the downloaded bytes are published to it so the archive can
    /// be read while it downloads. Segmented downloads publish the contiguous prefix as pieces
    /// land; every other path publishes the whole archive once it is on disk.
    pub(crate) fn download(
        &self,
        download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
        availability: Option<&ArchiveAvailability>,
    ) -> Result<DownloadedArchive> {
        let downloaded = self.download_archive(download_progress, availability)?;
        if let Some(availability) = availability {
            availability.complete(&downloaded)?;
        }
        Ok(downloaded)
    }

    /// Downloads the archive, publishing progress to `availability` only on the segmented path.
    fn download_archive(
        &self,
        download_progress: Option<&mut ArchiveDownloadProgress<'_>>,
        availability: Option<&ArchiveAvailability>,
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
                self.log_sequential_fallback(reason, probe.total_size);
                self.download_sequential(super::MAX_DOWNLOAD_RETRIES, download_progress)
            }
            FetchStrategy::Segmented(plan) => {
                self.download_segmented(probe.total_size, plan, download_progress, availability)
            }
        }
    }

    /// Removes any archive files created by this fetcher.
    pub(crate) fn cleanup_downloaded_files(&self) {
        self.paths.cleanup_all();
    }

    /// Probes the remote source for file size and HTTP range support.
    fn probe(&self, client: &BlockingClient) -> Result<RemoteArchiveProbe> {
        let mut attempt = 1;
        let probe = loop {
            let probe = client
                .get(&self.url)
                .header(RANGE, "bytes=0-0")
                .send()
                .and_then(|response| response.error_for_status());
            // A transient failure here would permanently downgrade the archive to one stream.
            if probe.is_ok() || attempt == SEGMENT_RETRY_ATTEMPTS {
                break probe
            }
            std::thread::sleep(self.session.retry_delay(piece_retry_backoff(attempt, false)));
            attempt += 1;
        };

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

        let total_size =
            total_size.ok_or_else(|| eyre::eyre!("Server did not return file size"))?;
        // A HEAD response reports a zero body length, so only a range probe gives the real size.
        // Without range support the download is sequential, which checks the size of the GET.
        if supports_ranges {
            self.check_total_size(total_size)?;
        }
        Ok(RemoteArchiveProbe { total_size, supports_ranges })
    }

    /// Fails when the server-reported archive size differs from the manifest-declared size.
    fn check_total_size(&self, total_size: u64) -> Result<()> {
        if let Some(expected) = self.expected_size &&
            expected != total_size
        {
            eyre::bail!(
                "Archive size mismatch: manifest declares {expected} bytes, server reported {total_size}"
            )
        }
        Ok(())
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
                            retry_delay = ?self.session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                            "Download failed, retrying"
                        );
                        std::thread::sleep(
                            self.session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                        );
                    }
                    continue;
                }
            };

            let is_partial = response.status() == StatusCode::PARTIAL_CONTENT;
            let content_range = response
                .headers()
                .get("Content-Range")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("bytes"))
                .map(str::trim);
            let size = if is_partial {
                content_range
                    .and_then(|value| value.split('/').next_back())
                    .and_then(|value| value.parse().ok())
            } else {
                response.content_length()
            };
            if let Some(size) = size {
                self.check_total_size(size)?;
            }

            // Appending a range that starts elsewhere would silently corrupt the `.part` file.
            let range_start = content_range
                .and_then(|value| value.split('-').next())
                .and_then(|value| value.parse::<u64>().ok());
            if is_partial && range_start != Some(existing_size) {
                // Resuming this `.part` file would fail the same way on the next run.
                self.paths.cleanup_partial();
                eyre::bail!(
                    "Server resumed {} at byte {} instead of requested offset {existing_size}",
                    self.paths.file_name(),
                    range_start.map_or_else(|| "unknown".to_string(), |start| start.to_string())
                )
            }

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
            // Never write past the archive's total size, whatever the server streams.
            let mut reader = response.take(current_total.saturating_sub(start_offset));

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
                        retry_delay = ?self.session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                        "Download interrupted, retrying"
                    );
                    std::thread::sleep(
                        self.session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                    );
                }
                continue;
            }

            let downloaded = fs::metadata(self.paths.part_path())?.len();
            if downloaded < current_total {
                // The body ended early without an error, so resume from what landed.
                last_error = Some(eyre::eyre!(
                    "Download of {} ended at {downloaded} of {current_total} bytes",
                    self.paths.file_name()
                ));
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
        availability: Option<&ArchiveAvailability>,
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
        .run(availability)
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
    ///
    /// Fails when the `.part` file does not hold exactly `size` bytes, so a short or overlong
    /// download never reaches extraction.
    fn finalize_download(&self, size: u64) -> Result<DownloadedArchive> {
        let downloaded = fs::metadata(self.paths.part_path())?.len();
        if downloaded != size {
            eyre::bail!(
                "Downloaded {} has {downloaded} bytes, expected {size}",
                self.paths.file_name()
            )
        }
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

/// The prefix of an archive that a fetch has written to disk, so extraction can read the
/// archive while it is still downloading.
#[derive(Default)]
pub(crate) struct ArchiveAvailability {
    /// Current fetch state.
    state: Mutex<AvailabilityState>,
    /// Wakes readers when the state changes.
    changed: Condvar,
}

/// Fetch state shared between one archive download and its reader.
#[derive(Default)]
enum AvailabilityState {
    /// The fetch has not opened the archive file yet.
    #[default]
    Pending,
    /// The first `available` of `total` bytes of `file` are written.
    Open {
        /// Archive file, shared with readers so a later rename or delete does not affect them.
        file: Arc<File>,
        /// Length of the written prefix.
        available: u64,
        /// Total archive size.
        total: u64,
    },
    /// No more bytes will arrive, because the fetch or the reader failed.
    Aborted,
}

impl ArchiveAvailability {
    /// Shares the archive file once it is created, with nothing written yet.
    fn open(&self, file: File, total: u64) {
        let mut state = self.lock();
        if !matches!(*state, AvailabilityState::Aborted) {
            *state = AvailabilityState::Open { file: Arc::new(file), available: 0, total };
            self.changed.notify_all();
        }
    }

    /// Publishes that the first `len` bytes are written. The prefix never shrinks.
    fn publish(&self, len: u64) {
        let mut state = self.lock();
        if let AvailabilityState::Open { available, total, .. } = &mut *state &&
            len > *available
        {
            *available = len.min(*total);
            self.changed.notify_all();
        }
    }

    /// Publishes the whole downloaded archive, opening it if the fetch path never did.
    fn complete(&self, downloaded: &DownloadedArchive) -> Result<()> {
        if matches!(*self.lock(), AvailabilityState::Pending) {
            self.open(fs::open(&downloaded.path)?, downloaded.size);
        }
        self.publish(downloaded.size);
        Ok(())
    }

    /// Fails readers and stops the fetch because the archive will not complete.
    ///
    /// Returns `true` if this call aborted the archive, and `false` if it was already aborted or
    /// fully published.
    pub(crate) fn abort(&self) -> bool {
        let mut state = self.lock();
        match *state {
            AvailabilityState::Aborted => false,
            AvailabilityState::Open { available, total, .. } if available == total => false,
            _ => {
                *state = AvailabilityState::Aborted;
                self.changed.notify_all();
                true
            }
        }
    }

    /// Returns a guard that aborts this availability when dropped, so a reader never blocks
    /// after the fetch returns early or unwinds.
    pub(crate) fn abort_on_drop(&self) -> ArchiveAvailabilityAbortGuard<'_> {
        ArchiveAvailabilityAbortGuard(self)
    }

    /// Returns whether the archive was aborted.
    fn is_aborted(&self) -> bool {
        matches!(*self.lock(), AvailabilityState::Aborted)
    }

    /// Blocks until bytes past `position` are written, then returns the file and the written
    /// length, which equals `position` only at the end of the archive.
    fn wait_past(&self, position: u64) -> io::Result<(Arc<File>, u64)> {
        let mut state = self.lock();
        loop {
            match &*state {
                AvailabilityState::Aborted => {
                    return Err(io::Error::other("archive download stopped before completing"))
                }
                AvailabilityState::Open { file, available, total }
                    if *available > position || available == total =>
                {
                    return Ok((Arc::clone(file), *available))
                }
                _ => state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner()),
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, AvailabilityState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Aborts an [`ArchiveAvailability`] when dropped.
pub(crate) struct ArchiveAvailabilityAbortGuard<'a>(&'a ArchiveAvailability);

impl Drop for ArchiveAvailabilityAbortGuard<'_> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Reads the published prefix of an archive, blocking at its end until more is written.
pub(crate) struct ArchiveAvailabilityReader<'a> {
    /// Availability published by the fetch.
    availability: &'a ArchiveAvailability,
    /// Next archive offset to read.
    position: u64,
}

impl<'a> ArchiveAvailabilityReader<'a> {
    /// Creates a reader starting at the beginning of the archive.
    pub(crate) const fn new(availability: &'a ArchiveAvailability) -> Self {
        Self { availability, position: 0 }
    }
}

impl Read for ArchiveAvailabilityReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0)
        }

        let (file, available) = self.availability.wait_past(self.position)?;
        let readable = usize::try_from(available - self.position).unwrap_or(usize::MAX);
        if readable == 0 {
            return Ok(0)
        }

        let len = buf.len().min(readable);
        let read = file.read_at(&mut buf[..len], self.position)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "archive is shorter than its published length",
            ))
        }
        self.position += read as u64;
        Ok(read)
    }
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
    /// Session carrying the retry-delay override.
    session: &'a DownloadSession,
    /// Receives the contiguous downloaded prefix, when the archive is read while downloading.
    availability: Option<&'a ArchiveAvailability>,
}

impl SegmentedWorkerContext<'_> {
    /// Returns why workers must stop: the command was cancelled, or the archive reader aborted.
    fn stop_error(&self) -> Option<eyre::Error> {
        if self.cancel_token.is_cancelled() {
            return Some(eyre::eyre!("Download cancelled"))
        }
        self.availability
            .is_some_and(ArchiveAvailability::is_aborted)
            .then(|| eyre::eyre!("Archive reader stopped before the download completed"))
    }

    /// Sleeps for `delay`, waking every 100ms to return early once workers must stop.
    fn backoff(&self, delay: Duration) {
        let deadline = Instant::now() + delay;
        while self.stop_error().is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return
            }
            std::thread::sleep(remaining.min(Duration::from_millis(100)));
        }
    }
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
    ///
    /// Publishes the contiguous downloaded prefix to `availability` as pieces land, and stops
    /// early once `availability` is aborted.
    fn run(self, availability: Option<&ArchiveAvailability>) -> Result<DownloadedArchive> {
        let Self { url, paths, total_size, plan, session } = self;
        {
            let file = fs::create_file(paths.part_path())?;
            file.set_len(total_size)?;
        }
        if let Some(availability) = availability {
            availability.open(fs::open(paths.part_path())?, total_size);
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
            session: &session,
            shared,
            request_limiter: request_limiter.as_ref(),
            cancel_token,
            availability,
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

        // Workers that stop between pieces exit without recording an error, so the file may be
        // incomplete even when no worker failed.
        let error = terminal_failure.take().or_else(|| worker_context.stop_error());
        if let Some(error) = error {
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

        while !context.availability.is_some_and(ArchiveAvailability::is_aborted) &&
            let Some(piece) = state.next_piece(context.cancel_token)
        {
            match Self::download_piece_with_retries(
                client,
                context,
                &file,
                piece,
                &piece_progress_bytes,
            ) {
                Ok(()) => {
                    let prefix = state.complete(piece);
                    if let Some(availability) = context.availability {
                        availability.publish(prefix);
                    }
                }
                Err(PieceAttemptFailure::Retryable { error, .. }) if state.requeue(piece) => {
                    warn!(target: "reth::cli",
                        start = piece.start,
                        end = piece.end,
                        %error,
                        "Piece download failed, requeued"
                    );
                }
                Err(
                    PieceAttemptFailure::Retryable { error, .. } |
                    PieceAttemptFailure::Terminal(error),
                ) => {
                    state.note_terminal_failure();
                    terminal_failure.record(error);
                    return;
                }
            }
        }
    }

    /// Downloads one queued piece with per-piece retry/backoff.
    ///
    /// Each attempt acquires a permit from the shared request limit so whole-file and
    /// piece downloads use the same fixed number of HTTP request slots. Stops between attempts,
    /// and during backoff, once the download is cancelled or the archive reader aborts.
    fn download_piece_with_retries(
        client: &BlockingClient,
        context: SegmentedWorkerContext<'_>,
        file: &std::fs::File,
        piece: DownloadPiece,
        piece_progress_bytes: &AtomicU64,
    ) -> std::result::Result<(), PieceAttemptFailure> {
        let SegmentedWorkerContext { url, shared, request_limiter, cancel_token, session, .. } =
            context;
        for attempt in 1..=SEGMENT_RETRY_ATTEMPTS {
            if let Some(error) = context.stop_error() {
                return Err(PieceAttemptFailure::Terminal(error))
            }

            let _request_permit = request_limiter
                .acquire(shared, cancel_token)
                .map_err(PieceAttemptFailure::Terminal)?;
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
                    context.backoff(session.retry_delay(piece_retry_backoff(attempt, throttled)));
                }
                Err(failure) => return Err(failure),
            }
        }

        Err(PieceAttemptFailure::Terminal(eyre::eyre!(
            "Piece download failed after {SEGMENT_RETRY_ATTEMPTS} attempts"
        )))
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

        let mut buf = vec![0u8; SEGMENTED_DOWNLOAD_WRITE_BUFFER_SIZE];
        let mut filled = 0;
        let mut reader = response.take(expected_len);
        // File offset of `buf[0]`.
        let mut offset = piece.start;

        loop {
            if cancel_token.is_cancelled() {
                return Err(PieceAttemptFailure::Terminal(eyre::eyre!("Download cancelled")));
            }

            match reader.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => {
                    filled += n;
                    if filled == buf.len() {
                        file.write_all_at(&buf, offset)
                            .map_err(|error| PieceAttemptFailure::Terminal(error.into()))?;
                        offset += filled as u64;
                        filled = 0;
                    }
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
        file.write_all_at(&buf[..filled], offset)
            .map_err(|error| PieceAttemptFailure::Terminal(error.into()))?;
        offset += filled as u64;

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
    /// Remaining number of times a piece that exhausted its retries may be requeued.
    requeues_left: AtomicU64,
    /// Downloaded pieces and the contiguous prefix they form.
    completed: Mutex<CompletedPieces>,
}

/// Downloaded pieces of one segmented archive.
#[derive(Default)]
struct CompletedPieces {
    /// Length of the archive prefix made only of downloaded pieces.
    prefix: u64,
    /// Exclusive end byte of each downloaded piece past `prefix`, keyed by start byte.
    pending: BTreeMap<u64, u64>,
}

impl SegmentedDownloadState {
    /// Creates the shared queue state for one segmented archive attempt.
    fn new(pieces: VecDeque<DownloadPiece>) -> Self {
        let requeues_left = AtomicU64::new(pieces.len() as u64);
        Self {
            pieces: Mutex::new(pieces),
            failed: AtomicBool::new(false),
            requeues_left,
            completed: Mutex::default(),
        }
    }

    /// Puts a piece back on the queue, returning `false` once the requeue budget is spent.
    ///
    /// The piece goes to the front so the contiguous prefix that extraction reads is not held
    /// back until the end of the archive.
    fn requeue(&self, piece: DownloadPiece) -> bool {
        if self
            .requeues_left
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1))
            .is_err()
        {
            return false
        }
        self.pieces.lock().unwrap().push_front(piece);
        true
    }

    /// Records a downloaded piece and returns the length of the contiguous downloaded prefix.
    fn complete(&self, piece: DownloadPiece) -> u64 {
        let mut guard = self.completed.lock().unwrap();
        let completed = &mut *guard;
        completed.pending.insert(piece.start, piece.end + 1);
        while let Some(end) = completed.pending.remove(&completed.prefix) {
            completed.prefix = end;
        }
        completed.prefix
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
    matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS) ||
        status.is_server_error()
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
pub(crate) fn panic_payload_message(payload: Box<dyn Any + Send + 'static>) -> String {
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
        io::{BufRead, Write},
        net::TcpListener,
    };

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
        assert!(should_retry_piece_status(StatusCode::from_u16(520).unwrap()));
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

        let downloaded = fetcher.download(None, None).unwrap();

        assert_eq!(downloaded.path, archive_path);
        assert_eq!(downloaded.size, b"local archive bytes".len() as u64);
        assert!(!cache_dir.join("state.tar.zst").exists());
        assert!(!cache_dir.join("state.tar.zst.part").exists());
    }

    #[test]
    fn segmented_state_requeues_failed_pieces_within_budget() {
        let state = SegmentedDownloadState::new(build_download_pieces(8, 4));
        let token = CancellationToken::new();
        let first = state.next_piece(&token).unwrap();

        assert!(state.requeue(first));
        assert!(state.requeue(first));
        assert!(!state.requeue(first));
        assert_eq!(state.pieces.lock().unwrap().front(), Some(&first));
    }

    #[test]
    fn segmented_state_prefix_stops_at_first_missing_piece() {
        let pieces = build_download_pieces(10, 4).into_iter().collect::<Vec<_>>();
        let state = SegmentedDownloadState::new(VecDeque::new());

        assert_eq!(state.complete(pieces[2]), 0);
        assert_eq!(state.complete(pieces[0]), 4);
        assert_eq!(state.complete(pieces[1]), 10);
    }

    #[test]
    fn availability_reader_follows_published_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.part");
        let data = (0..=255u8).cycle().take(1000).collect::<Vec<_>>();
        std::fs::write(&path, &data).unwrap();
        let availability = ArchiveAvailability::default();

        let read = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let mut read = Vec::new();
                ArchiveAvailabilityReader::new(&availability).read_to_end(&mut read).map(|_| read)
            });
            availability.open(fs::open(&path).unwrap(), data.len() as u64);
            for len in [300, 299, 1000] {
                availability.publish(len);
            }
            reader.join().unwrap()
        });

        assert_eq!(read.unwrap(), data);
        assert!(!availability.abort(), "a fully published archive stays readable");
    }

    #[test]
    fn availability_reader_fails_when_download_aborts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.part");
        std::fs::write(&path, [7u8; 100]).unwrap();
        let availability = ArchiveAvailability::default();
        availability.open(fs::open(&path).unwrap(), 100);
        availability.publish(40);

        let mut reader = ArchiveAvailabilityReader::new(&availability);
        let mut buf = [0u8; 100];
        assert_eq!(reader.read(&mut buf).unwrap(), 40);
        assert!(availability.abort());

        let error = reader.read(&mut buf).unwrap_err();
        assert_eq!(error.to_string(), "archive download stopped before completing");
    }

    #[test]
    fn segmented_download_publishes_downloaded_prefix() {
        let data = (0..=255u8).cycle().take(1024 * 1024 + 123).collect::<Vec<_>>();
        let (url, _) = spawn_range_server(data.clone(), false);
        let dir = tempfile::tempdir().unwrap();
        let availability = ArchiveAvailability::default();

        let (downloaded, read) = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let mut read = Vec::new();
                ArchiveAvailabilityReader::new(&availability).read_to_end(&mut read).map(|_| read)
            });
            let downloaded = {
                let _abort_guard = availability.abort_on_drop();
                test_segmented_download(&url, dir.path(), data.len() as u64, test_session())
                    .run(Some(&availability))
            };
            (downloaded, reader.join().unwrap())
        });

        assert_eq!(downloaded.unwrap().size, data.len() as u64);
        assert_eq!(read.unwrap(), data);
    }

    #[test]
    fn segmented_download_stops_once_reader_aborted() {
        let (url, requests) = spawn_range_server(vec![0; 1024 * 1024], false);
        let dir = tempfile::tempdir().unwrap();
        let availability = ArchiveAvailability::default();
        assert!(availability.abort());

        let error = test_segmented_download(&url, dir.path(), 1024 * 1024, test_session())
            .run(Some(&availability))
            .unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "Parallel download failed: Archive reader stopped before the download completed"
        );
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn segmented_download_backoff_stops_once_reader_aborts() {
        let (url, requests) = spawn_range_server(Vec::new(), true);
        let dir = tempfile::tempdir().unwrap();
        let session = test_session().with_retry_backoff(Some(Duration::from_secs(30)));
        let availability = ArchiveAvailability::default();
        let started = Instant::now();

        let result = std::thread::scope(|scope| {
            let download = scope.spawn(|| {
                test_segmented_download(&url, dir.path(), 1024 * 1024, session)
                    .run(Some(&availability))
            });
            while requests.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(availability.abort());
            download.join().unwrap()
        });

        assert_eq!(
            format!("{:#}", result.unwrap_err()),
            "Parallel download failed: Archive reader stopped before the download completed"
        );
        // Without stopping during backoff, workers would sleep through two 30s retry delays.
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn sequential_download_rejects_size_other_than_expected() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\n\
                         01234567890123456789";
        let url = spawn_response_server(response.to_vec());
        let dir = tempfile::tempdir().unwrap();
        let session = DownloadSession::new(None, None, CancellationToken::new());
        let fetcher = ArchiveFetcher::new(url, dir.path(), session, Some(10));

        let error = fetcher.download(None, None).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "Archive size mismatch: manifest declares 10 bytes, server reported 20"
        );
        assert!(!dir.path().join("archive.tar.zst").exists());
    }

    #[test]
    fn sequential_resume_rejects_range_not_starting_at_offset() {
        let response = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/10\r\n\
                         Content-Length: 10\r\nConnection: close\r\n\r\n0123456789";
        let url = spawn_response_server(response.to_vec());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("archive.tar.zst.part"), b"01234").unwrap();
        let session = DownloadSession::new(None, None, CancellationToken::new());
        let fetcher = ArchiveFetcher::new(url, dir.path(), session, Some(10));

        let error = fetcher.download(None, None).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "Server resumed archive.tar.zst at byte 0 instead of requested offset 5"
        );
        assert!(!dir.path().join("archive.tar.zst").exists());
    }

    #[test]
    fn sequential_download_stops_at_declared_size() {
        let response = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-9/10\r\n\
                         Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                         14\r\n01234567890123456789\r\n0\r\n\r\n";
        let url = spawn_response_server(response.to_vec());
        let dir = tempfile::tempdir().unwrap();
        let session = DownloadSession::new(None, None, CancellationToken::new());
        let fetcher = ArchiveFetcher::new(url, dir.path(), session, Some(10));

        let downloaded = fetcher.download(None, None).unwrap();

        assert_eq!(downloaded.size, 10);
        assert_eq!(std::fs::read(downloaded.path).unwrap(), b"0123456789");
    }

    #[test]
    fn download_without_range_support_accepts_expected_size() {
        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789";
        let url = spawn_response_server(response.to_vec());
        let dir = tempfile::tempdir().unwrap();
        let fetcher = ArchiveFetcher::new(url, dir.path(), test_session(), Some(10));

        let downloaded = fetcher.download(None, None).unwrap();

        assert_eq!(std::fs::read(downloaded.path).unwrap(), b"0123456789");
    }

    #[test]
    fn probe_rejects_size_other_than_expected() {
        let response = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/20\r\n\
                         Content-Length: 1\r\nConnection: close\r\n\r\n0";
        let url = spawn_response_server(response.to_vec());
        let dir = tempfile::tempdir().unwrap();
        let fetcher = ArchiveFetcher::new(url, dir.path(), test_session(), Some(10));

        let error = fetcher.download(None, None).unwrap_err();

        assert_eq!(
            format!("{error:#}"),
            "Archive size mismatch: manifest declares 10 bytes, server reported 20"
        );
        assert!(!dir.path().join("archive.tar.zst").exists());
    }

    fn test_session() -> DownloadSession {
        DownloadSession::new(None, Some(DownloadRequestLimiter::new(4)), CancellationToken::new())
    }

    /// Plans 64 KiB pieces so a small archive still exercises several workers.
    fn test_segmented_download(
        url: &str,
        dir: &Path,
        total_size: u64,
        session: DownloadSession,
    ) -> SegmentedDownload {
        let piece_size = 64 * 1024;
        let pieces = build_download_pieces(total_size, piece_size);
        let plan = SegmentedDownloadPlan {
            piece_size,
            piece_count: pieces.len(),
            worker_count: 4,
            pieces,
        };
        SegmentedDownload::new(
            url.to_string(),
            DownloadPaths::from_url(url, dir),
            total_size,
            plan,
            session,
            None,
        )
    }

    /// Serves `data` over HTTP on localhost with range support, or answers every request with
    /// 503 when `unavailable` is set. Returns the URL and the number of requests received.
    fn spawn_range_server(data: Vec<u8>, unavailable: bool) -> (String, Arc<AtomicU64>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/archive.tar.zst", listener.local_addr().unwrap());
        let data = Arc::new(data);
        let requests = Arc::new(AtomicU64::new(0));
        let received = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let (data, received) = (Arc::clone(&data), Arc::clone(&received));
                std::thread::spawn(move || {
                    let mut stream = stream.unwrap();
                    let mut request = io::BufReader::new(stream.try_clone().unwrap());
                    let mut range = None;
                    loop {
                        let mut line = String::new();
                        if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break
                        }
                        if let Some(value) = line.to_ascii_lowercase().strip_prefix("range: bytes=")
                        {
                            let (start, end) = value.trim().split_once('-').unwrap();
                            range = Some((
                                start.parse::<usize>().unwrap(),
                                end.parse::<usize>().unwrap(),
                            ));
                        }
                    }
                    received.fetch_add(1, Ordering::SeqCst);

                    if unavailable {
                        stream
                            .write_all(
                                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\
                                  Connection: close\r\n\r\n",
                            )
                            .unwrap();
                        return
                    }
                    let (start, end) = range.unwrap();
                    let body = &data[start..=end];
                    write!(
                        stream,
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len(),
                        body.len()
                    )
                    .unwrap();
                    stream.write_all(body).unwrap();
                });
            }
        });
        (url, requests)
    }

    /// Answers every HTTP request on localhost with the raw `response` bytes and returns the URL.
    fn spawn_response_server(response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/archive.tar.zst", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = io::BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if request.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break
                    }
                }
                stream.write_all(&response).unwrap();
            }
        });
        url
    }
}
