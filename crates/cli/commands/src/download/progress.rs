use eyre::Result;
use reth_cli_util::cancellation::CancellationToken;
use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use tracing::info;

const BYTE_UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];

/// Tracks download progress and throttles display updates to every 100ms.
pub(crate) struct DownloadProgress {
    /// Bytes copied so far for this single download.
    pub(crate) downloaded: u64,
    /// Total bytes expected for this single download.
    total_size: u64,
    /// Time when the progress line was last printed.
    last_displayed: Instant,
    /// Time when this progress tracker started.
    started_at: Instant,
}

impl DownloadProgress {
    /// Creates new progress tracker with given total size
    pub(crate) fn new(total_size: u64) -> Self {
        let now = Instant::now();
        Self { downloaded: 0, total_size, last_displayed: now, started_at: now }
    }

    /// Converts bytes to human readable format (B, KB, MB, GB)
    pub(crate) fn format_size(size: u64) -> String {
        let mut size = size as f64;
        let mut unit_index = 0;

        while size >= 1024.0 && unit_index < BYTE_UNITS.len() - 1 {
            size /= 1024.0;
            unit_index += 1;
        }

        format!("{:.2} {}", size, BYTE_UNITS[unit_index])
    }

    /// Format duration as human readable string
    pub(crate) fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else if secs < 3600 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }

    /// Updates progress bar (for single-archive legacy downloads)
    pub(crate) fn update(&mut self, chunk_size: u64) -> Result<()> {
        self.downloaded += chunk_size;

        if self.last_displayed.elapsed() >= Duration::from_millis(100) {
            let formatted_downloaded = Self::format_size(self.downloaded);
            let formatted_total = Self::format_size(self.total_size);
            let progress = (self.downloaded as f64 / self.total_size as f64) * 100.0;

            let elapsed = self.started_at.elapsed();
            let eta = if self.downloaded > 0 {
                let remaining = self.total_size.saturating_sub(self.downloaded);
                let speed = self.downloaded as f64 / elapsed.as_secs_f64();
                if speed > 0.0 {
                    Duration::from_secs_f64(remaining as f64 / speed)
                } else {
                    Duration::ZERO
                }
            } else {
                Duration::ZERO
            };
            let eta_str = Self::format_duration(eta);

            print!(
                "\rDownloading and extracting... {progress:.2}% ({formatted_downloaded} / {formatted_total}) ETA: {eta_str}     ",
            );
            io::stdout().flush()?;
            self.last_displayed = Instant::now();
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct PhaseStart {
    started_at: Instant,
    baseline_bytes: u64,
}

/// Shared progress counters for parallel downloads.
pub(crate) struct SharedProgress {
    /// Raw HTTP bytes fetched during this session, including retries.
    pub(crate) session_fetched_bytes: AtomicU64,
    /// Compressed bytes from archives that have fully downloaded.
    pub(crate) completed_download_bytes: AtomicU64,
    /// Compressed bytes written for currently active archive download attempts.
    pub(crate) active_download_bytes: AtomicU64,
    /// Total compressed bytes expected across all planned archives.
    pub(crate) total_download_bytes: u64,
    /// Plain-output bytes from archives that have fully verified.
    pub(crate) completed_output_bytes: AtomicU64,
    /// Plain-output bytes unpacked by currently active extractions.
    pub(crate) active_extracted_output_bytes: AtomicU64,
    /// Plain-output bytes hashed by currently active verifications.
    pub(crate) active_verified_output_bytes: AtomicU64,
    /// Total plain-output bytes expected across all planned archives.
    pub(crate) total_output_bytes: u64,
    /// Total number of planned archives.
    pub(crate) total_archives: u64,
    /// Time when the modular download job started.
    pub(crate) started_at: Instant,
    /// Time and baseline when the current extraction phase started.
    extraction_phase: Mutex<Option<PhaseStart>>,
    /// Time and baseline when the current verification phase started.
    verification_phase: Mutex<Option<PhaseStart>>,
    /// Number of archives that have fully finished.
    pub(crate) archives_done: AtomicU64,
    /// Number of archives currently in the fetch phase.
    pub(crate) active_downloads: AtomicU64,
    /// Number of in-flight HTTP requests.
    pub(crate) active_download_requests: AtomicU64,
    /// Number of archives currently extracting.
    pub(crate) active_extractions: AtomicU64,
    /// Number of archives currently verifying extracted outputs.
    pub(crate) active_verifications: AtomicU64,
    /// Signals the background progress task to exit.
    pub(crate) done: AtomicBool,
    /// Cancellation token shared by the whole command.
    cancel_token: CancellationToken,
}

impl SharedProgress {
    /// Creates the shared progress state for a modular download job.
    pub(crate) fn new(
        total_download_bytes: u64,
        total_output_bytes: u64,
        total_archives: u64,
        cancel_token: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_fetched_bytes: AtomicU64::new(0),
            completed_download_bytes: AtomicU64::new(0),
            active_download_bytes: AtomicU64::new(0),
            total_download_bytes,
            completed_output_bytes: AtomicU64::new(0),
            active_extracted_output_bytes: AtomicU64::new(0),
            active_verified_output_bytes: AtomicU64::new(0),
            total_output_bytes,
            total_archives,
            started_at: Instant::now(),
            extraction_phase: Mutex::new(None),
            verification_phase: Mutex::new(None),
            archives_done: AtomicU64::new(0),
            active_downloads: AtomicU64::new(0),
            active_download_requests: AtomicU64::new(0),
            active_extractions: AtomicU64::new(0),
            active_verifications: AtomicU64::new(0),
            done: AtomicBool::new(false),
            cancel_token,
        })
    }

    /// Returns whether the whole command has been cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Adds raw session traffic bytes without affecting logical progress.
    pub(crate) fn record_session_fetched_bytes(&self, bytes: u64) {
        self.session_fetched_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn add_active_download_bytes(&self, bytes: u64) {
        self.active_download_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn sub_active_download_bytes(&self, bytes: u64) {
        sub_bytes(&self.active_download_bytes, bytes);
    }

    fn add_active_extracted_output_bytes(&self, bytes: u64) {
        self.active_extracted_output_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn sub_active_extracted_output_bytes(&self, bytes: u64) {
        sub_bytes(&self.active_extracted_output_bytes, bytes);
    }

    fn add_active_verified_output_bytes(&self, bytes: u64) {
        self.active_verified_output_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    fn sub_active_verified_output_bytes(&self, bytes: u64) {
        sub_bytes(&self.active_verified_output_bytes, bytes);
    }

    /// Records an archive whose outputs were already present locally.
    pub(crate) fn record_reused_archive(&self, download_bytes: u64, output_bytes: u64) {
        self.completed_download_bytes.fetch_add(download_bytes, Ordering::Relaxed);
        self.completed_output_bytes.fetch_add(output_bytes, Ordering::Relaxed);
        self.archives_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an archive whose compressed download completed successfully.
    pub(crate) fn record_archive_download_complete(&self, bytes: u64) {
        self.completed_download_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Records an archive whose extracted outputs have fully verified.
    pub(crate) fn record_archive_output_complete(&self, bytes: u64) {
        self.completed_output_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.archives_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns logical compressed download progress.
    pub(crate) fn logical_downloaded_bytes(&self) -> u64 {
        (self.completed_download_bytes.load(Ordering::Relaxed) +
            self.active_download_bytes.load(Ordering::Relaxed))
        .min(self.total_download_bytes)
    }

    /// Returns verified plain-output bytes.
    pub(crate) fn verified_output_bytes(&self) -> u64 {
        self.completed_output_bytes.load(Ordering::Relaxed).min(self.total_output_bytes)
    }

    /// Returns plain-output bytes currently represented by extraction progress.
    pub(crate) fn extracting_output_bytes(&self) -> u64 {
        (self.completed_output_bytes.load(Ordering::Relaxed) +
            self.active_extracted_output_bytes.load(Ordering::Relaxed))
        .min(self.total_output_bytes)
    }

    /// Returns plain-output bytes currently represented by verification progress.
    pub(crate) fn verifying_output_bytes(&self) -> u64 {
        (self.completed_output_bytes.load(Ordering::Relaxed) +
            self.active_verified_output_bytes.load(Ordering::Relaxed))
        .min(self.total_output_bytes)
    }

    fn restart_phase(slot: &Mutex<Option<PhaseStart>>, baseline_bytes: u64) {
        *slot.lock().unwrap() = Some(PhaseStart { started_at: Instant::now(), baseline_bytes });
    }

    fn phase_eta(
        slot: &Mutex<Option<PhaseStart>>,
        current_bytes: u64,
        total_bytes: u64,
    ) -> Option<Duration> {
        let phase = *slot.lock().unwrap();
        let phase = phase?;
        let done = current_bytes.saturating_sub(phase.baseline_bytes);
        let total = total_bytes.saturating_sub(phase.baseline_bytes);
        eta_from_progress(phase.started_at.elapsed(), done, total)
    }

    fn extraction_eta(&self, current_bytes: u64) -> Option<Duration> {
        Self::phase_eta(&self.extraction_phase, current_bytes, self.total_output_bytes)
    }

    fn verification_eta(&self, current_bytes: u64) -> Option<Duration> {
        Self::phase_eta(&self.verification_phase, current_bytes, self.total_output_bytes)
    }

    /// Marks one archive as actively downloading.
    pub(crate) fn download_started(&self) {
        self.active_downloads.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks one archive download as finished.
    pub(crate) fn download_finished(&self) {
        sub_bytes(&self.active_downloads, 1);
    }

    /// Marks one HTTP request as in flight.
    pub(crate) fn request_started(&self) {
        self.active_download_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Marks one HTTP request as finished.
    pub(crate) fn request_finished(&self) {
        sub_bytes(&self.active_download_requests, 1);
    }

    /// Marks one archive as actively extracting.
    pub(crate) fn extraction_started(&self) {
        if self.active_extractions.fetch_add(1, Ordering::Relaxed) == 0 {
            Self::restart_phase(
                &self.extraction_phase,
                self.completed_output_bytes.load(Ordering::Relaxed),
            );
        }
    }

    /// Marks one archive extraction as finished.
    pub(crate) fn extraction_finished(&self) {
        sub_bytes(&self.active_extractions, 1);
    }

    /// Marks one archive as actively verifying outputs.
    pub(crate) fn verification_started(&self) {
        if self.active_verifications.fetch_add(1, Ordering::Relaxed) == 0 {
            Self::restart_phase(
                &self.verification_phase,
                self.completed_output_bytes.load(Ordering::Relaxed),
            );
        }
    }

    /// Marks one archive verification as finished.
    pub(crate) fn verification_finished(&self) {
        sub_bytes(&self.active_verifications, 1);
    }
}

fn sub_bytes(counter: &AtomicU64, bytes: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(bytes))
    });
}

fn eta_from_progress(elapsed: Duration, done: u64, total: u64) -> Option<Duration> {
    if done == 0 || done >= total {
        return None;
    }

    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }

    let speed = done as f64 / secs;
    if speed <= 0.0 {
        return None;
    }

    Some(Duration::from_secs_f64((total - done) as f64 / speed))
}

fn format_percent(done: u64, total: u64) -> String {
    if total == 0 {
        return "100.0%".to_string();
    }

    format!("{:.1}%", (done as f64 / total as f64) * 100.0)
}

fn format_eta(eta: Option<Duration>) -> String {
    eta.map(DownloadProgress::format_duration).unwrap_or_else(|| "unknown".to_string())
}

/// Global request limit for the blocking downloader.
///
/// This uses `Mutex + Condvar` because the segmented path runs blocking reqwest
/// clients on OS threads.
pub(crate) struct DownloadRequestLimiter {
    /// Maximum number of in-flight HTTP requests.
    limit: usize,
    /// Current number of acquired request slots.
    active: Mutex<usize>,
    /// Wakes blocked threads when a slot is released.
    notify: Condvar,
}

impl DownloadRequestLimiter {
    /// Creates the shared request limiter.
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self { limit: limit.max(1), active: Mutex::new(0), notify: Condvar::new() })
    }

    /// Returns the configured request limit.
    pub(crate) fn max_concurrency(&self) -> usize {
        self.limit
    }

    pub(crate) fn acquire<'a>(
        &'a self,
        progress: Option<&'a Arc<SharedProgress>>,
        cancel_token: &CancellationToken,
    ) -> Result<DownloadRequestPermit<'a>> {
        let mut active = self.active.lock().unwrap();
        loop {
            if cancel_token.is_cancelled() {
                return Err(eyre::eyre!("Download cancelled"));
            }

            if *active < self.limit {
                *active += 1;
                if let Some(progress) = progress {
                    progress.request_started();
                }
                return Ok(DownloadRequestPermit { limiter: self, progress });
            }

            // Wake periodically so cancellation can interrupt waiters even if
            // no request finishes.
            let (next_active, _) =
                self.notify.wait_timeout(active, Duration::from_millis(100)).unwrap();
            active = next_active;
        }
    }
}

/// RAII permit for one in-flight HTTP request.
///
/// Dropping the permit releases a slot in the shared request limit and updates
/// the live progress counters.
pub(crate) struct DownloadRequestPermit<'a> {
    /// Limiter that owns the request slot.
    limiter: &'a DownloadRequestLimiter,
    /// Shared progress counters updated when the permit drops.
    progress: Option<&'a Arc<SharedProgress>>,
}

impl Drop for DownloadRequestPermit<'_> {
    /// Releases the request slot and updates shared progress counters.
    fn drop(&mut self) {
        let mut active = self.limiter.active.lock().unwrap();
        *active = active.saturating_sub(1);
        drop(active);
        self.limiter.notify.notify_one();

        if let Some(progress) = self.progress {
            progress.request_finished();
        }
    }
}

/// Tracks one active archive download attempt.
pub(crate) struct ArchiveDownloadProgress<'a> {
    progress: Option<&'a Arc<SharedProgress>>,
    downloaded: u64,
    completed: bool,
}

impl<'a> ArchiveDownloadProgress<'a> {
    /// Starts tracking one archive download attempt.
    pub(crate) fn new(progress: Option<&'a Arc<SharedProgress>>) -> Self {
        if let Some(progress) = progress {
            progress.download_started();
        }
        Self { progress, downloaded: 0, completed: false }
    }

    /// Adds logical compressed bytes written by this attempt.
    pub(crate) fn record_downloaded(&mut self, bytes: u64) {
        self.downloaded += bytes;
        if let Some(progress) = self.progress {
            progress.add_active_download_bytes(bytes);
        }
    }

    /// Returns whether this tracker has recorded any logical bytes itself.
    pub(crate) fn has_tracked_bytes(&self) -> bool {
        self.downloaded > 0
    }

    /// Moves this archive from active download bytes into completed download bytes.
    pub(crate) fn complete(&mut self, total_bytes: u64) {
        if self.completed {
            return;
        }
        if let Some(progress) = self.progress {
            progress.sub_active_download_bytes(self.downloaded);
            progress.record_archive_download_complete(total_bytes);
        }
        self.downloaded = 0;
        self.completed = true;
    }
}

impl Drop for ArchiveDownloadProgress<'_> {
    fn drop(&mut self) {
        if let Some(progress) = self.progress {
            progress.sub_active_download_bytes(self.downloaded);
            progress.download_finished();
        }
    }
}

/// Tracks one active archive extraction attempt.
pub(crate) struct ArchiveExtractionProgress {
    progress: Option<Arc<SharedProgress>>,
    extracted: Arc<AtomicU64>,
    finished: bool,
}

/// Cloneable handle for reporting extracted bytes from background monitoring.
#[derive(Clone)]
pub(crate) struct ArchiveExtractionProgressHandle {
    progress: Arc<SharedProgress>,
    extracted: Arc<AtomicU64>,
}

impl ArchiveExtractionProgress {
    /// Starts tracking one archive extraction attempt.
    pub(crate) fn new(progress: Option<&Arc<SharedProgress>>) -> Self {
        if let Some(progress) = progress {
            progress.extraction_started();
        }
        Self {
            progress: progress.cloned(),
            extracted: Arc::new(AtomicU64::new(0)),
            finished: false,
        }
    }

    /// Returns a cloneable handle that can report extraction progress from another thread.
    pub(crate) fn handle(&self) -> Option<ArchiveExtractionProgressHandle> {
        Some(ArchiveExtractionProgressHandle {
            progress: Arc::clone(self.progress.as_ref()?),
            extracted: Arc::clone(&self.extracted),
        })
    }

    /// Adds plain-output bytes extracted by this attempt.
    pub(crate) fn record_extracted(&mut self, bytes: u64) {
        if let Some(handle) = self.handle() {
            handle.record_extracted(bytes);
        }
    }

    /// Ends extraction tracking before verification begins.
    pub(crate) fn finish(&mut self) {
        if self.finished {
            return;
        }
        if let Some(progress) = &self.progress {
            progress.sub_active_extracted_output_bytes(self.extracted.swap(0, Ordering::Relaxed));
        }
        self.finished = true;
    }
}

impl Drop for ArchiveExtractionProgress {
    fn drop(&mut self) {
        if let Some(progress) = &self.progress {
            progress.sub_active_extracted_output_bytes(self.extracted.swap(0, Ordering::Relaxed));
            progress.extraction_finished();
        }
    }
}

impl ArchiveExtractionProgressHandle {
    /// Adds plain-output bytes extracted by this attempt.
    pub(crate) fn record_extracted(&self, bytes: u64) {
        self.extracted.fetch_add(bytes, Ordering::Relaxed);
        self.progress.add_active_extracted_output_bytes(bytes);
    }
}

/// Tracks one active archive verification attempt.
pub(crate) struct ArchiveVerificationProgress<'a> {
    progress: Option<&'a Arc<SharedProgress>>,
    verified: u64,
    completed: bool,
}

impl<'a> ArchiveVerificationProgress<'a> {
    /// Starts tracking one archive verification attempt.
    pub(crate) fn new(progress: Option<&'a Arc<SharedProgress>>) -> Self {
        if let Some(progress) = progress {
            progress.verification_started();
        }
        Self { progress, verified: 0, completed: false }
    }

    /// Adds plain-output bytes hashed by this verification attempt.
    pub(crate) fn record_verified(&mut self, bytes: u64) {
        self.verified += bytes;
        if let Some(progress) = self.progress {
            progress.add_active_verified_output_bytes(bytes);
        }
    }

    /// Moves this archive from active verification bytes into completed output bytes.
    pub(crate) fn complete(&mut self, total_bytes: u64) {
        if self.completed {
            return;
        }
        if let Some(progress) = self.progress {
            progress.sub_active_verified_output_bytes(self.verified);
            progress.record_archive_output_complete(total_bytes);
        }
        self.verified = 0;
        self.completed = true;
    }
}

impl Drop for ArchiveVerificationProgress<'_> {
    fn drop(&mut self) {
        if let Some(progress) = self.progress {
            progress.sub_active_verified_output_bytes(self.verified);
            progress.verification_finished();
        }
    }
}

/// Adapter to track progress while reading (used for extraction in legacy path)
pub(crate) struct ProgressReader<R> {
    /// Wrapped reader that provides archive bytes.
    reader: R,
    /// Per-download progress tracker for legacy paths.
    progress: DownloadProgress,
    /// Cancellation token checked between reads.
    cancel_token: CancellationToken,
}

impl<R: Read> ProgressReader<R> {
    /// Wraps a reader with per-download progress tracking.
    pub(crate) fn new(reader: R, total_size: u64, cancel_token: CancellationToken) -> Self {
        Self { reader, progress: DownloadProgress::new(total_size), cancel_token }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    /// Reads bytes, checks cancellation, and updates the local progress bar.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel_token.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let bytes = self.reader.read(buf)?;
        if bytes > 0 &&
            let Err(error) = self.progress.update(bytes as u64)
        {
            return Err(io::Error::other(error));
        }
        Ok(bytes)
    }
}

/// Wrapper that bumps a shared atomic counter while writing data.
/// Used for parallel downloads where a single display task shows aggregated progress.
pub(crate) struct SharedProgressWriter<'a, W> {
    /// Wrapped writer receiving downloaded bytes.
    pub(crate) inner: W,
    /// Shared counters updated as bytes are written.
    pub(crate) progress: Arc<SharedProgress>,
    /// Optional callback for logical bytes written by the current archive attempt.
    pub(crate) on_written: Option<&'a mut dyn FnMut(u64)>,
}

impl<W: Write> Write for SharedProgressWriter<'_, W> {
    /// Writes bytes and records them in shared progress.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.write(buf)?;
        self.progress.record_session_fetched_bytes(n as u64);
        if let Some(on_written) = self.on_written.as_deref_mut() {
            on_written(n as u64);
        }
        Ok(n)
    }

    /// Flushes the wrapped writer.
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Wrapper that bumps a shared atomic counter while reading data.
/// Used for streaming downloads where a single display task shows aggregated progress.
pub(crate) struct SharedProgressReader<R> {
    /// Wrapped reader producing streamed bytes.
    pub(crate) inner: R,
    /// Shared counters updated as bytes are read.
    pub(crate) progress: Arc<SharedProgress>,
}

impl<R: Read> Read for SharedProgressReader<R> {
    /// Reads bytes and records them in shared progress.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.progress.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "download cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.progress.record_session_fetched_bytes(n as u64);
        Ok(n)
    }
}

/// Spawns a background task that prints aggregated download progress.
/// Returns a handle; drop it (or call `.abort()`) to stop.
pub(crate) fn spawn_progress_display(progress: Arc<SharedProgress>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3));
        interval.tick().await;
        loop {
            interval.tick().await;

            if progress.done.load(Ordering::Relaxed) {
                break;
            }

            let download_total = progress.total_download_bytes;
            let output_total = progress.total_output_bytes;
            if download_total == 0 && output_total == 0 {
                continue;
            }

            let done = progress.archives_done.load(Ordering::Relaxed);
            let all = progress.total_archives;
            let active_downloads = progress.active_downloads.load(Ordering::Relaxed);
            let active_requests = progress.active_download_requests.load(Ordering::Relaxed);
            let active_extractions = progress.active_extractions.load(Ordering::Relaxed);
            let active_verifications = progress.active_verifications.load(Ordering::Relaxed);
            let downloaded = progress.logical_downloaded_bytes();
            let extracted = progress.extracting_output_bytes();
            let verified = progress.verifying_output_bytes();
            let elapsed = DownloadProgress::format_duration(progress.started_at.elapsed());
            let download_total_display = DownloadProgress::format_size(download_total);
            let output_total_display = DownloadProgress::format_size(output_total);
            let downloaded_display = DownloadProgress::format_size(downloaded);
            let extracted_display = DownloadProgress::format_size(extracted);
            let active_download_phase = active_downloads > 0 || active_requests > 0;

            if active_download_phase {
                info!(target: "reth::cli",
                    archives = format_args!("{done}/{all}"),
                    progress = %format_percent(downloaded, download_total),
                    elapsed = %elapsed,
                    eta = %format_eta(eta_from_progress(progress.started_at.elapsed(), downloaded, download_total)),
                    bytes = format_args!("{downloaded_display}/{download_total_display}"),
                    "Downloading snapshot archives"
                );
            } else if active_extractions > 0 {
                info!(target: "reth::cli",
                    archives = format_args!("{done}/{all}"),
                    progress = %format_percent(extracted, output_total),
                    elapsed = %elapsed,
                    eta = %format_eta(progress.extraction_eta(extracted)),
                    bytes = format_args!("{extracted_display}/{output_total_display}"),
                    "Extracting snapshot archives"
                );
            } else if active_verifications > 0 {
                info!(target: "reth::cli",
                    archives = format_args!("{done}/{all}"),
                    progress = %format_percent(verified, output_total),
                    elapsed = %elapsed,
                    eta = %format_eta(progress.verification_eta(verified)),
                    bytes = format_args!("{}/{output_total_display}", DownloadProgress::format_size(verified)),
                    "Verifying snapshot archives"
                );
            } else {
                continue;
            }
        }

        let completed = progress.verified_output_bytes();
        let completed_display = DownloadProgress::format_size(completed);
        let output_total = DownloadProgress::format_size(progress.total_output_bytes);
        info!(target: "reth::cli",
            archives = format_args!("{}/{}", progress.total_archives, progress.total_archives),
            progress = "100.0%",
            elapsed = %DownloadProgress::format_duration(progress.started_at.elapsed()),
            eta = "0s",
            bytes = format_args!("{completed_display}/{output_total}"),
            "Snapshot archive processing complete"
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn shared_progress_separates_session_fetch_from_logical_progress() {
        let progress = SharedProgress::new(10, 20, 1, CancellationToken::new());

        progress.record_session_fetched_bytes(10);
        progress.record_session_fetched_bytes(10);
        progress.record_archive_download_complete(10);
        progress.record_archive_output_complete(20);

        assert_eq!(progress.session_fetched_bytes.load(Ordering::Relaxed), 20);
        assert_eq!(progress.logical_downloaded_bytes(), 10);
        assert_eq!(progress.verified_output_bytes(), 20);
        assert_eq!(progress.archives_done.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn archive_download_progress_rolls_back_unfinished_attempts() {
        let progress = SharedProgress::new(10, 20, 1, CancellationToken::new());

        {
            let mut download = ArchiveDownloadProgress::new(Some(&progress));
            download.record_downloaded(4);
            assert_eq!(progress.logical_downloaded_bytes(), 4);
        }

        assert_eq!(progress.logical_downloaded_bytes(), 0);
        assert_eq!(progress.active_downloads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn extraction_phase_baseline_restarts_after_idle() {
        let progress = SharedProgress::new(10, 100, 1, CancellationToken::new());

        progress.extraction_started();
        assert_eq!(progress.extraction_phase.lock().unwrap().as_ref().unwrap().baseline_bytes, 0);

        progress.completed_output_bytes.store(25, Ordering::Relaxed);
        progress.extraction_started();
        assert_eq!(progress.extraction_phase.lock().unwrap().as_ref().unwrap().baseline_bytes, 0);

        progress.extraction_finished();
        progress.extraction_finished();
        progress.extraction_started();
        assert_eq!(progress.extraction_phase.lock().unwrap().as_ref().unwrap().baseline_bytes, 25);
    }

    #[test]
    fn verification_phase_baseline_restarts_after_idle() {
        let progress = SharedProgress::new(10, 100, 1, CancellationToken::new());

        progress.verification_started();
        assert_eq!(progress.verification_phase.lock().unwrap().as_ref().unwrap().baseline_bytes, 0);

        progress.completed_output_bytes.store(40, Ordering::Relaxed);
        progress.verification_started();
        assert_eq!(progress.verification_phase.lock().unwrap().as_ref().unwrap().baseline_bytes, 0);

        progress.verification_finished();
        progress.verification_finished();
        progress.verification_started();
        assert_eq!(
            progress.verification_phase.lock().unwrap().as_ref().unwrap().baseline_bytes,
            40
        );
    }
}
