use super::{
    fetch::{ArchiveFetcher, DownloadedArchive},
    progress::{
        ArchiveExtractionProgress, ArchiveExtractionProgressHandle, DownloadProgress,
        DownloadRequestLimiter, ProgressReader, SharedProgressReader,
    },
    session::DownloadSession,
    verify::ExtractedHashes,
    MAX_DOWNLOAD_RETRIES, RETRY_BACKOFF_SECS,
};
use eyre::{Result, WrapErr};
use lz4::Decoder;
use reqwest::blocking::Client as BlockingClient;
use reth_cli_util::cancellation::CancellationToken;
use reth_fs_util as fs;
use std::{
    cell::RefCell,
    collections::HashSet,
    io::{self, Read},
    mem,
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, SyncSender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tar::Archive;
use tracing::{info, warn};
use url::Url;
use zstd::stream::read::Decoder as ZstdDecoder;

const EXTENSION_TAR_LZ4: &str = ".tar.lz4";
const EXTENSION_TAR_ZSTD: &str = ".tar.zst";
const STREAMING_EXTRACTION_PROGRESS_MIN_FILE_SIZE: u64 = 64 * 1024 * 1024;
const EXTRACTION_PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Extracted file bytes sent to the hashing thread at a time.
const FILE_HASH_BATCH_SIZE: usize = 1 << 20;

/// Batches buffered for the hashing thread before extraction waits for it.
const FILE_HASH_QUEUE_LEN: usize = 16;

/// Supported compression formats for snapshots
#[derive(Debug, Clone, Copy)]
pub(crate) enum CompressionFormat {
    /// LZ4-compressed tar archive.
    Lz4,
    /// Zstandard-compressed tar archive.
    Zstd,
}

impl CompressionFormat {
    /// Detect compression format from file extension
    pub(crate) fn from_url(url: &str) -> Result<Self> {
        let path =
            Url::parse(url).map(|u| u.path().to_string()).unwrap_or_else(|_| url.to_string());

        if path.ends_with(EXTENSION_TAR_LZ4) {
            Ok(Self::Lz4)
        } else if path.ends_with(EXTENSION_TAR_ZSTD) {
            Ok(Self::Zstd)
        } else {
            Err(eyre::eyre!(
                "Unsupported file format. Expected .tar.lz4 or .tar.zst, got: {}",
                path
            ))
        }
    }
}

/// Extracts a compressed tar archive to the target directory with progress tracking.
fn extract_archive<R: Read>(
    reader: R,
    total_size: u64,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let progress_reader = ProgressReader::new(reader, total_size, cancel_token);

    match format {
        CompressionFormat::Lz4 => {
            let decoder = Decoder::new(progress_reader)?;
            unpack_archive(Archive::new(decoder), target_dir, static_files_dir, None, None)?;
        }
        CompressionFormat::Zstd => {
            let decoder = ZstdDecoder::new(progress_reader)?;
            unpack_archive(Archive::new(decoder), target_dir, static_files_dir, None, None)?;
        }
    }

    println!();
    Ok(())
}

/// Extracts a compressed tar archive without progress tracking.
pub(crate) fn extract_archive_raw<R: Read>(
    reader: R,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    match format {
        CompressionFormat::Lz4 => {
            unpack_archive(
                Archive::new(Decoder::new(reader)?),
                target_dir,
                static_files_dir,
                progress,
                None,
            )?;
        }
        CompressionFormat::Zstd => {
            unpack_archive(
                Archive::new(ZstdDecoder::new(reader)?),
                target_dir,
                static_files_dir,
                progress,
                None,
            )?;
        }
    }

    Ok(())
}

/// Extracts a compressed tar archive like [`extract_archive_raw`], hashing each declared output
/// written as a regular file from the bytes written to it so verification does not have to read
/// it back. `outputs` holds the destination paths of the archive's declared outputs.
pub(crate) fn extract_archive_hashed<R: Read>(
    reader: R,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    outputs: HashSet<PathBuf>,
    progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<ExtractedHashes> {
    let hasher = RefCell::new(FileHasher::spawn(outputs));
    match format {
        CompressionFormat::Lz4 => unpack_archive(
            Archive::new(FileHashTee {
                inner: Decoder::new(reader)?,
                position: 0,
                hasher: &hasher,
            }),
            target_dir,
            static_files_dir,
            progress,
            Some(&hasher),
        )?,
        CompressionFormat::Zstd => unpack_archive(
            Archive::new(FileHashTee {
                inner: ZstdDecoder::new(reader)?,
                position: 0,
                hasher: &hasher,
            }),
            target_dir,
            static_files_dir,
            progress,
            Some(&hasher),
        )?,
    }
    hasher.into_inner().join()
}

fn unpack_archive<R: Read>(
    mut archive: Archive<R>,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    mut progress: Option<&mut ArchiveExtractionProgress>,
    hasher: Option<&RefCell<FileHasher>>,
) -> Result<()> {
    // Every entry goes through `unpack_entry` so link entries are rejected; `Archive::unpack`
    // would create them.
    let entries = archive.entries().wrap_err_with(|| {
        format!("failed to read archive entries for `{}`", target_dir.display())
    })?;

    for entry in entries {
        let mut entry = entry.wrap_err_with(|| {
            format!("failed to read archive entry for `{}`", target_dir.display())
        })?;
        let hashing = match hasher {
            Some(hasher) => {
                hasher.borrow_mut().start_entry(&entry, target_dir, static_files_dir)?
            }
            None => false,
        };
        extract_entry_with_progress(
            &mut entry,
            target_dir,
            static_files_dir,
            progress.as_deref_mut(),
        )?;
        if hashing && let Some(hasher) = hasher {
            hasher.borrow_mut().finish_file()?;
        }
    }

    Ok(())
}

/// Passes archive bytes from the decompressor to the tar reader, feeding them to a
/// [`FileHasher`] on the way.
struct FileHashTee<'a, R> {
    /// Decompressed tar stream.
    inner: R,
    /// Offset in the tar stream of the next byte read, which matches tar's own entry offsets.
    position: u64,
    /// Hasher of the file being extracted.
    hasher: &'a RefCell<FileHasher>,
}

impl<R: Read> Read for FileHashTee<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.hasher.borrow_mut().feed(self.position, &buf[..read])?;
        self.position += read as u64;
        Ok(read)
    }
}

/// Hashes the data of each regular file as tar extracts it, on a separate thread so hashing
/// stays off the extraction thread.
struct FileHasher {
    /// Tar stream offsets of the current file's data; empty when no file is being hashed.
    range: Range<u64>,
    /// Bytes of the current file seen so far.
    seen: u64,
    /// Current file bytes not yet sent to the hashing thread.
    batch: Vec<u8>,
    /// Sends work to the hashing thread.
    sender: SyncSender<FileHashMessage>,
    /// Hashing thread, which returns the hashes of every completed file.
    thread: thread::JoinHandle<ExtractedHashes>,
    /// Destination paths of the archive's declared outputs; other entries are not hashed, so
    /// the recorded hashes are bounded by the manifest rather than the archive's entry count.
    outputs: HashSet<PathBuf>,
}

/// Work for the [`FileHasher`] thread.
enum FileHashMessage {
    /// Starts hashing the file extracted to this path.
    Start(PathBuf),
    /// Next bytes of the current file.
    Bytes(Vec<u8>),
    /// Records the current file's hash.
    Finish,
    /// Drops the current file because not all of its bytes were seen.
    Discard,
    /// Drops every recorded hash, because an entry replaces a file that an earlier entry may
    /// have written.
    ForgetAll,
}

impl FileHasher {
    /// Starts the hashing thread for an archive with the given declared output paths.
    fn spawn(outputs: HashSet<PathBuf>) -> Self {
        let (sender, receiver) = sync_channel(FILE_HASH_QUEUE_LEN);
        let thread = thread::spawn(move || {
            let mut hashes = ExtractedHashes::default();
            let mut current = None;
            for message in receiver {
                match message {
                    FileHashMessage::Start(path) => current = Some((path, blake3::Hasher::new())),
                    FileHashMessage::Bytes(bytes) => {
                        if let Some((_, hasher)) = &mut current {
                            hasher.update(&bytes);
                        }
                    }
                    FileHashMessage::Finish => {
                        if let Some((path, hasher)) = current.take() {
                            hashes.insert(path, hasher.count(), hasher.finalize());
                        }
                    }
                    FileHashMessage::Discard => current = None,
                    FileHashMessage::ForgetAll => hashes.clear(),
                }
            }
            hashes
        });
        Self {
            range: 0..0,
            seen: 0,
            batch: Vec::with_capacity(FILE_HASH_BATCH_SIZE),
            sender,
            thread,
            outputs,
        }
    }

    /// Starts hashing `entry` if it is a regular file at a declared output path, and returns
    /// whether it did.
    ///
    /// Other entries, such as links, sparse files and undeclared paths, get no hash, so their
    /// outputs are verified by reading them back.
    fn start_entry<R: Read>(
        &mut self,
        entry: &tar::Entry<'_, R>,
        target_dir: &Path,
        static_files_dir: Option<&Path>,
    ) -> Result<bool> {
        let Some(path) = entry_destination_path(entry, target_dir, static_files_dir)? else {
            return Ok(false)
        };
        // This entry replaces a file already at the destination. An earlier entry may have
        // written it under another spelling of the path, such as a different case on a
        // case-insensitive filesystem, so which recorded hash it had is unknown and none is kept.
        if std::fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_dir()) {
            self.send(FileHashMessage::ForgetAll)?;
        }
        if !self.outputs.contains(&path) {
            return Ok(false)
        }
        let entry_type = entry.header().entry_type();
        if !entry_type.is_file() && !entry_type.is_contiguous() {
            return Ok(false)
        }
        let start = entry.raw_file_position();
        self.range = start..start + entry.size();
        self.seen = 0;
        self.send(FileHashMessage::Start(path))?;
        Ok(true)
    }

    /// Hashes the bytes of the current file among `bytes`, which start at tar stream offset
    /// `position`.
    fn feed(&mut self, position: u64, bytes: &[u8]) -> io::Result<()> {
        let start = self.range.start.max(position);
        let end = self.range.end.min(position + bytes.len() as u64);
        if start >= end {
            return Ok(())
        }
        self.seen += end - start;
        let mut bytes = &bytes[(start - position) as usize..(end - position) as usize];
        // Batches are filled exactly, so a batch never outgrows its allocation.
        while !bytes.is_empty() {
            let (head, tail) =
                bytes.split_at(bytes.len().min(FILE_HASH_BATCH_SIZE - self.batch.len()));
            self.batch.extend_from_slice(head);
            bytes = tail;
            if self.batch.len() == FILE_HASH_BATCH_SIZE {
                self.send_batch()?;
            }
        }
        Ok(())
    }

    /// Records the current file's hash once all of its bytes were seen, and discards it
    /// otherwise.
    fn finish_file(&mut self) -> io::Result<()> {
        self.send_batch()?;
        let complete = self.seen == self.range.end - self.range.start;
        self.range = 0..0;
        self.send(if complete { FileHashMessage::Finish } else { FileHashMessage::Discard })
    }

    /// Waits for the hashing thread and returns the hashes of every completed file.
    fn join(self) -> Result<ExtractedHashes> {
        drop(self.sender);
        self.thread.join().map_err(|_| eyre::eyre!("Extracted file hashing thread panicked"))
    }

    fn send_batch(&mut self) -> io::Result<()> {
        if self.batch.is_empty() {
            return Ok(())
        }
        let batch = mem::replace(&mut self.batch, Vec::with_capacity(FILE_HASH_BATCH_SIZE));
        self.send(FileHashMessage::Bytes(batch))
    }

    fn send(&self, message: FileHashMessage) -> io::Result<()> {
        self.sender.send(message).map_err(|_| io::Error::other("extracted file hashing stopped"))
    }
}

/// Returns the path within the static files directory, accepting tar's optional `./` prefix.
pub(crate) fn static_file_relative_path(path: &Path) -> Option<&Path> {
    path.strip_prefix(".").unwrap_or(path).strip_prefix("static_files").ok()
}

/// Fails if any existing component of `relative_path` below `root` is a symlink.
///
/// Components that do not exist yet are ignored. `root` itself is trusted and not checked. This
/// is a point-in-time check: it does not stop a link from being created right after it returns.
pub(crate) fn ensure_no_symlinks(root: &Path, relative_path: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    for part in relative_path.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => eyre::ensure!(
                !metadata.file_type().is_symlink(),
                "Archive path `{}` contains a symlink",
                current.display()
            ),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

/// Extracts static files beneath their configured root without allowing archive paths or links
/// to escape it.
fn unpack_static_file<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    static_files_dir: &Path,
    relative_path: &Path,
) -> Result<()> {
    eyre::ensure!(
        relative_path
            .components()
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        "Invalid static file archive path: {}",
        relative_path.display()
    );
    let entry_type = entry.header().entry_type();
    eyre::ensure!(
        entry_type.is_file() || entry_type.is_dir(),
        "Unsupported static file archive entry"
    );
    fs::create_dir_all(static_files_dir)?;
    let root = static_files_dir.to_path_buf();
    let dest = root.join(relative_path);
    // Check each existing ancestor before creating directories or replacing an output.
    ensure_no_symlinks(&root, relative_path)?;
    if relative_path.as_os_str().is_empty() {
        return Ok(())
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    entry.unpack(dest)?;
    Ok(())
}

fn unpack_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
) -> Result<()> {
    // Links planted by an archive could redirect later extraction, or retry cleanup running
    // concurrently for another archive, outside the datadir. Snapshots never contain links.
    let entry_type = entry.header().entry_type();
    eyre::ensure!(
        !entry_type.is_symlink() && !entry_type.is_hard_link(),
        "Archive links are unsupported: `{}`",
        entry.path()?.display()
    );
    let path = entry.path()?.into_owned();
    if let Some(static_files_dir) = static_files_dir &&
        let Some(relative_path) = static_file_relative_path(&path)
    {
        unpack_static_file(entry, static_files_dir, relative_path)
    } else {
        entry.unpack_in(target_dir)?;
        Ok(())
    }
}

fn extract_entry_with_progress<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    let size = entry.header().entry_size().unwrap_or(0);
    let entry_type = entry.header().entry_type();

    if !entry_type.is_file() || size == 0 {
        unpack_entry(entry, target_dir, static_files_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    }

    if size < STREAMING_EXTRACTION_PROGRESS_MIN_FILE_SIZE {
        unpack_entry(entry, target_dir, static_files_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        if let Some(progress) = progress {
            progress.record_extracted(size);
        }
        return Ok(())
    }

    let Some(progress_handle) = progress.as_ref().and_then(|progress| progress.handle()) else {
        unpack_entry(entry, target_dir, static_files_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    };

    let Some(entry_path) = entry_destination_path(entry, target_dir, static_files_dir)? else {
        unpack_entry(entry, target_dir, static_files_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    };

    let stop = Arc::new(AtomicBool::new(false));
    let monitor = spawn_extraction_progress_monitor(entry_path, progress_handle, Arc::clone(&stop));
    let unpack_result = unpack_entry(entry, target_dir, static_files_dir)
        .wrap_err_with(|| format!("failed to extract archive into `{}`", target_dir.display()));
    stop.store(true, Ordering::Relaxed);

    let monitor_result = monitor.join();
    unpack_result?;

    monitor_result.map_err(|_| eyre::eyre!("extraction progress monitor panicked"))?;
    Ok(())
}

fn entry_destination_path<R: Read>(
    entry: &tar::Entry<'_, R>,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let mut file_dst = target_dir.to_path_buf();
    let path = entry.path().wrap_err("invalid path in archive entry")?;
    let path = if let Some(static_files_dir) = static_files_dir &&
        let Some(relative_path) = static_file_relative_path(&path)
    {
        file_dst = static_files_dir.to_path_buf();
        relative_path
    } else {
        path.as_ref()
    };

    for part in path.components() {
        match part {
            Component::Prefix(..) | Component::RootDir | Component::CurDir => continue,
            Component::ParentDir => return Ok(None),
            Component::Normal(part) => file_dst.push(part),
        }
    }

    if file_dst == target_dir {
        return Ok(None)
    }

    Ok(Some(file_dst))
}

fn spawn_extraction_progress_monitor(
    entry_path: PathBuf,
    progress: ArchiveExtractionProgressHandle,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut extracted = 0_u64;

        loop {
            record_extracted_file_bytes(&entry_path, &progress, &mut extracted);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(EXTRACTION_PROGRESS_POLL_INTERVAL);
        }
    })
}

fn record_extracted_file_bytes(
    entry_path: &Path,
    progress: &ArchiveExtractionProgressHandle,
    extracted: &mut u64,
) {
    let Ok(meta) = fs::metadata(entry_path) else { return };
    let len = meta.len();
    if len > *extracted {
        progress.record_extracted(len - *extracted);
        *extracted = len;
    }
}

/// Extracts a snapshot from a local file.
fn extract_from_file(
    path: &Path,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let total_size = file.metadata()?.len();
    info!(target: "reth::cli",
        file = %path.display(),
        size = %DownloadProgress::format_size(total_size),
        "Extracting local archive"
    );
    let start = Instant::now();
    extract_archive(
        file,
        total_size,
        format,
        target_dir,
        static_files_dir,
        CancellationToken::new(),
    )?;
    info!(target: "reth::cli",
        file = %path.display(),
        elapsed = %DownloadProgress::format_duration(start.elapsed()),
        "Local extraction complete"
    );
    Ok(())
}

/// Streams a remote archive directly into the extractor without writing to disk.
///
/// On failure, retries from scratch up to [`MAX_DOWNLOAD_RETRIES`] times.
pub(crate) fn streaming_download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    session: &DownloadSession,
) -> Result<()> {
    if let Some(path) = archive_file_url_path(url)? {
        let size = path.metadata()?.len();
        extract_from_file(&path, format, target_dir, static_files_dir)?;
        session.record_archive_output_complete(size);
        return Ok(())
    }

    let shared = session.progress();
    let quiet = session.progress().is_some();
    let mut last_error: Option<eyre::Error> = None;

    for attempt in 1..=MAX_DOWNLOAD_RETRIES {
        if attempt > 1 {
            info!(target: "reth::cli",
                url = %url,
                attempt,
                max = MAX_DOWNLOAD_RETRIES,
                "Retrying streaming download from scratch"
            );
        }

        let client = BlockingClient::builder().connect_timeout(Duration::from_secs(30)).build()?;
        let _request_permit = session
            .request_limiter()
            .map(|limiter| limiter.acquire(session.progress(), session.cancel_token()))
            .transpose()?;

        let response = match client.get(url).send().and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(error) => {
                let err = eyre::Error::from(error);
                if attempt < MAX_DOWNLOAD_RETRIES {
                    warn!(target: "reth::cli",
                        url = %url,
                        attempt,
                        max = MAX_DOWNLOAD_RETRIES,
                        err = %err,
                        "Streaming request failed, retrying"
                    );
                }
                last_error = Some(err);
                if attempt < MAX_DOWNLOAD_RETRIES {
                    std::thread::sleep(
                        session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                    );
                }
                continue;
            }
        };

        if !quiet && let Some(size) = response.content_length() {
            info!(target: "reth::cli",
                url = %url,
                size = %DownloadProgress::format_size(size),
                "Streaming archive"
            );
        }

        let result = if let Some(progress) = shared {
            let reader = SharedProgressReader { inner: response, progress: Arc::clone(progress) };
            extract_archive_raw(reader, format, target_dir, static_files_dir, None)
        } else {
            let total_size = response.content_length().unwrap_or(0);
            extract_archive(
                response,
                total_size,
                format,
                target_dir,
                static_files_dir,
                session.cancel_token().clone(),
            )
        };

        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt < MAX_DOWNLOAD_RETRIES {
                    warn!(target: "reth::cli",
                        url = %url,
                        attempt,
                        max = MAX_DOWNLOAD_RETRIES,
                        err = %error,
                        "Streaming extraction failed, retrying"
                    );
                }
                last_error = Some(error);
                if attempt < MAX_DOWNLOAD_RETRIES {
                    std::thread::sleep(
                        session.retry_delay(Duration::from_secs(RETRY_BACKOFF_SECS)),
                    );
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        eyre::eyre!("Streaming download failed after {MAX_DOWNLOAD_RETRIES} attempts")
    }))
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

/// Fetches the snapshot from a remote URL with resume support, then extracts it.
fn download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    session: DownloadSession,
) -> Result<()> {
    let quiet = session.progress().is_some();
    let fetcher = ArchiveFetcher::new(url.to_string(), target_dir, session.clone());
    let DownloadedArchive { path: downloaded_path, size: total_size } =
        fetcher.download(None, None)?;

    let file_name =
        downloaded_path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();

    if !quiet {
        info!(target: "reth::cli",
            file = %file_name,
            size = %DownloadProgress::format_size(total_size),
            "Extracting archive"
        );
    }
    let file = fs::open(&downloaded_path)?;

    if quiet {
        extract_archive_raw(file, format, target_dir, static_files_dir, None)?;
    } else {
        extract_archive(
            file,
            total_size,
            format,
            target_dir,
            static_files_dir,
            session.cancel_token().clone(),
        )?;
        info!(target: "reth::cli",
            file = %file_name,
            "Extraction complete"
        );
    }

    fetcher.cleanup_downloaded_files();
    session.record_archive_output_complete(total_size);

    Ok(())
}

/// Downloads and extracts a snapshot, blocking until finished.
///
/// Supports `file://` URLs for local files and HTTP(S) URLs for remote downloads.
/// When `resumable` is true, downloads to a `.part` file first with HTTP Range resume
/// support. Otherwise streams directly into the extractor.
fn blocking_download_and_extract(
    url: &str,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    resumable: bool,
    request_limiter: Option<Arc<DownloadRequestLimiter>>,
    cancel_token: CancellationToken,
    retry_backoff: Option<Duration>,
) -> Result<()> {
    let format = CompressionFormat::from_url(url)?;

    if let Ok(parsed_url) = Url::parse(url) &&
        parsed_url.scheme() == "file"
    {
        let session = DownloadSession::new(None, request_limiter, cancel_token)
            .with_retry_backoff(retry_backoff);
        let file_path = parsed_url
            .to_file_path()
            .map_err(|_| eyre::eyre!("Invalid file:// URL path: {}", url))?;
        let result = extract_from_file(&file_path, format, target_dir, static_files_dir);
        if result.is_ok() {
            session.record_archive_output_complete(file_path.metadata()?.len());
        }
        result
    } else if let Some(request_limiter) = request_limiter {
        download_and_extract(
            url,
            format,
            target_dir,
            static_files_dir,
            DownloadSession::new(None, Some(request_limiter), cancel_token)
                .with_retry_backoff(retry_backoff),
        )
    } else if resumable {
        let session =
            DownloadSession::new(None, Some(DownloadRequestLimiter::new(1)), cancel_token)
                .with_retry_backoff(retry_backoff);
        download_and_extract(url, format, target_dir, static_files_dir, session)
    } else {
        let session =
            DownloadSession::new(None, None, cancel_token).with_retry_backoff(retry_backoff);
        let result =
            streaming_download_and_extract(url, format, target_dir, static_files_dir, &session);
        if result.is_ok() {
            session.record_archive_output_complete(0);
        }
        result
    }
}

/// Downloads and extracts a snapshot archive asynchronously.
///
/// Download progress is reported using a local progress bar.
/// When `resumable` is true, uses two-phase download with `.part` files.
pub(crate) async fn stream_and_extract(
    url: &str,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    resumable: bool,
    request_limiter: Option<Arc<DownloadRequestLimiter>>,
    cancel_token: CancellationToken,
    retry_backoff: Option<Duration>,
) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let static_files_dir = static_files_dir.map(Path::to_path_buf);
    let url = url.to_string();
    reth_tasks::spawn_blocking(move || {
        blocking_download_and_extract(
            &url,
            &target_dir,
            static_files_dir.as_deref(),
            resumable,
            request_limiter,
            cancel_token,
            retry_backoff,
        )
    })
    .await??;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_static_root_cannot_be_redirected_by_archive_symlinks() {
        for custom_path in ["custom", "custom/nested"] {
            let target = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            fs::create_dir_all(outside.path().join("nested")).unwrap();
            for path in ["headers", "nested/headers"] {
                fs::write(outside.path().join(path), b"keep").unwrap();
            }
            let custom = target.path().join(custom_path);
            let mut archive = tar::Builder::new(Vec::new());
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            archive.append_link(&mut link, "custom", outside.path()).unwrap();
            let mut file = tar::Header::new_gnu();
            file.set_size(4);
            file.set_mode(0o644);
            file.set_cksum();
            archive.append_data(&mut file, "static_files/headers", b"data".as_slice()).unwrap();
            let tar = archive.into_inner().unwrap();
            let err = unpack_archive(
                Archive::new(tar.as_slice()),
                target.path(),
                Some(&custom),
                None,
                None,
            )
            .unwrap_err();
            assert!(format!("{err:#}").contains("Archive links are unsupported"));
            let outputs = [super::super::manifest::OutputFileChecksum {
                path: "static_files/headers".into(),
                size: 4,
                blake3: String::new(),
            }];
            super::super::verify::OutputVerifier::new(target.path(), Some(&custom))
                .cleanup(&outputs);
            for path in ["headers", "nested/headers"] {
                assert_eq!(fs::read(outside.path().join(path)).unwrap(), b"keep");
            }
        }
    }

    #[test]
    fn link_entries_are_rejected_without_custom_static_dir() {
        for entry_type in [tar::EntryType::Symlink, tar::EntryType::Link] {
            for with_progress in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let target = root.path().join("target");
                let outside = root.path().join("outside");
                fs::create_dir_all(&target).unwrap();
                fs::create_dir_all(&outside).unwrap();
                fs::write(outside.join("file"), b"keep").unwrap();

                let mut archive = tar::Builder::new(Vec::new());
                let mut link = tar::Header::new_gnu();
                link.set_entry_type(entry_type);
                link.set_size(0);
                link.set_mode(0o777);
                // A symlink to the outside directory would redirect the entry below, a hard link
                // would alias the outside file.
                let link_target =
                    if entry_type.is_symlink() { outside.clone() } else { outside.join("file") };
                archive.append_link(&mut link, "db", link_target).unwrap();
                let mut file = tar::Header::new_gnu();
                file.set_size(4);
                file.set_mode(0o644);
                file.set_cksum();
                archive.append_data(&mut file, "db/file", b"data".as_slice()).unwrap();
                let tar = archive.into_inner().unwrap();

                let mut progress = ArchiveExtractionProgress::new(None);
                let progress = with_progress.then_some(&mut progress);
                let err =
                    unpack_archive(Archive::new(tar.as_slice()), &target, None, progress, None)
                        .unwrap_err();
                assert!(format!("{err:#}").contains("Archive links are unsupported"));

                assert!(
                    !target.join("db").exists() && target.join("db").symlink_metadata().is_err()
                );
                assert_eq!(fs::read(outside.join("file")).unwrap(), b"keep");
                assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
            }
        }
    }

    #[test]
    fn extracted_hashes_replace_read_back_only_for_regular_files() {
        let checksum = |path: &str, data: &[u8]| super::super::manifest::OutputFileChecksum {
            path: path.into(),
            size: data.len() as u64,
            blake3: blake3::hash(data).to_hex().to_string(),
        };
        let data = (0..3 * FILE_HASH_BATCH_SIZE + 5).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let mut archive = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, "./db/data", data.as_slice()).unwrap();
        archive.append_data(&mut header, "db/extra", data.as_slice()).unwrap();
        let tar = zstd::encode_all(archive.into_inner().unwrap().as_slice(), 0).unwrap();
        let target = tempfile::tempdir().unwrap();

        let hashes = extract_archive_hashed(
            tar.as_slice(),
            CompressionFormat::Zstd,
            target.path(),
            None,
            HashSet::from([target.path().join("db/data")]),
            None,
        )
        .unwrap();

        // A file the archive did not write has no extraction hash, so it is read back.
        fs::write(target.path().join("db/other"), &data).unwrap();
        let verifier = super::super::verify::OutputVerifier::new(target.path(), None);
        let outputs = [checksum("db/data", &data), checksum("db/other", &data)];
        assert!(verifier.verify_with_progress(&outputs, None, Some(&hashes)).unwrap());
        let missing = [checksum("db/data", &data), checksum("db/missing", b"")];
        assert!(
            !verifier.verify_with_progress(&missing, None, Some(&hashes)).unwrap(),
            "an output the archive never produced still fails"
        );

        // Corrupt the files on disk: the extracted declared file keeps its extraction hash, while
        // the other file and the undeclared entry are read back and see the corruption.
        let mut corrupt = data.clone();
        corrupt[7] ^= 1;
        fs::write(target.path().join("db/data"), &corrupt).unwrap();
        fs::write(target.path().join("db/other"), &corrupt).unwrap();
        fs::write(target.path().join("db/extra"), &corrupt).unwrap();
        assert!(verifier.verify_with_progress(&outputs[..1], None, Some(&hashes)).unwrap());
        assert!(!verifier.verify_with_progress(&outputs[1..], None, Some(&hashes)).unwrap());
        assert!(!verifier
            .verify_with_progress(&[checksum("db/extra", &data)], None, Some(&hashes))
            .unwrap());

        let mut wrong = outputs[..1].to_vec();
        wrong[0].blake3 = blake3::hash(&corrupt).to_hex().to_string();
        assert!(
            !verifier.verify_with_progress(&wrong, None, Some(&hashes)).unwrap(),
            "extraction hashes are compared with the manifest"
        );

        // Static files in a custom directory are found by their manifest path.
        let mut archive = tar::Builder::new(Vec::new());
        archive.append_data(&mut header, "./static_files/headers", data.as_slice()).unwrap();
        let tar = zstd::encode_all(archive.into_inner().unwrap().as_slice(), 0).unwrap();
        let custom = tempfile::tempdir().unwrap();
        let hashes = extract_archive_hashed(
            tar.as_slice(),
            CompressionFormat::Zstd,
            target.path(),
            Some(custom.path()),
            HashSet::from([custom.path().join("headers")]),
            None,
        )
        .unwrap();
        fs::write(custom.path().join("headers"), &corrupt).unwrap();
        let verifier =
            super::super::verify::OutputVerifier::new(target.path(), Some(custom.path()));
        assert!(verifier
            .verify_with_progress(&[checksum("static_files/headers", &data)], None, Some(&hashes))
            .unwrap());

        // An entry that replaces a file written by an earlier entry drops the recorded hashes,
        // whether it is hashed or not; tar writes unknown entry types as regular files. On a
        // case-insensitive filesystem `db/DATA` replaces `db/data` as well.
        for (path, entry_type) in
            [("db/data", tar::EntryType::Fifo), ("db/DATA", tar::EntryType::Regular)]
        {
            let mut archive = tar::Builder::new(Vec::new());
            header.set_entry_type(tar::EntryType::Regular);
            archive.append_data(&mut header, "db/data", data.as_slice()).unwrap();
            header.set_entry_type(entry_type);
            archive.append_data(&mut header, path, corrupt.as_slice()).unwrap();
            let tar = zstd::encode_all(archive.into_inner().unwrap().as_slice(), 0).unwrap();
            let target = tempfile::tempdir().unwrap();
            let hashes = extract_archive_hashed(
                tar.as_slice(),
                CompressionFormat::Zstd,
                target.path(),
                None,
                HashSet::from([target.path().join("db/data")]),
                None,
            )
            .unwrap();
            let replaced = fs::read(target.path().join("db/data")).unwrap() == corrupt;
            assert!(replaced || entry_type == tar::EntryType::Regular);
            let verifier = super::super::verify::OutputVerifier::new(target.path(), None);
            assert_eq!(
                verifier
                    .verify_with_progress(&[checksum("db/data", &data)], None, Some(&hashes))
                    .unwrap(),
                !replaced,
                "`{path}` replaced the hashed file: {replaced}"
            );
        }

        // An output the archive made unreadable is an error, as it is for a read-back, unless
        // the user can read it regardless.
        #[cfg(unix)]
        {
            let mut archive = tar::Builder::new(Vec::new());
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o200);
            archive.append_data(&mut header, "db/data", data.as_slice()).unwrap();
            let tar = zstd::encode_all(archive.into_inner().unwrap().as_slice(), 0).unwrap();
            let target = tempfile::tempdir().unwrap();
            let hashes = extract_archive_hashed(
                tar.as_slice(),
                CompressionFormat::Zstd,
                target.path(),
                None,
                HashSet::from([target.path().join("db/data")]),
                None,
            )
            .unwrap();
            let readable = fs::open(target.path().join("db/data")).is_ok();
            let verifier = super::super::verify::OutputVerifier::new(target.path(), None);
            let verified =
                verifier.verify_with_progress(&[checksum("db/data", &data)], None, Some(&hashes));
            assert_eq!(verified.is_ok(), readable);
        }
    }

    #[test]
    fn remap_static_files_in_both_compression_formats() {
        let mut archive = tar::Builder::new(Vec::new());
        let mut directory = tar::Header::new_gnu();
        directory.set_entry_type(tar::EntryType::Directory);
        directory.set_size(0);
        directory.set_mode(0o755);
        directory.set_cksum();
        archive.append_data(&mut directory, "./static_files/", std::io::empty()).unwrap();
        for path in ["./static_files/nested/headers", "db/data"] {
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, path, b"data".as_slice()).unwrap();
        }
        let tar = archive.into_inner().unwrap();
        for format in [CompressionFormat::Lz4, CompressionFormat::Zstd] {
            let bytes = match format {
                CompressionFormat::Lz4 => {
                    let mut encoder = lz4::EncoderBuilder::new().build(Vec::new()).unwrap();
                    std::io::copy(&mut tar.as_slice(), &mut encoder).unwrap();
                    let (bytes, result) = encoder.finish();
                    result.unwrap();
                    bytes
                }
                CompressionFormat::Zstd => zstd::encode_all(tar.as_slice(), 0).unwrap(),
            };
            let target = tempfile::tempdir().unwrap();
            let custom = tempfile::tempdir().unwrap();
            extract_archive_raw(bytes.as_slice(), format, target.path(), Some(custom.path()), None)
                .unwrap();
            assert_eq!(fs::read(custom.path().join("nested/headers")).unwrap(), b"data");
            assert_eq!(fs::read(target.path().join("db/data")).unwrap(), b"data");
            assert!(!target.path().join("static_files").exists());
        }
    }

    #[test]
    fn test_compression_format_detection() {
        assert!(matches!(
            CompressionFormat::from_url("https://example.com/snapshot.tar.lz4"),
            Ok(CompressionFormat::Lz4)
        ));
        assert!(matches!(
            CompressionFormat::from_url("https://example.com/snapshot.tar.zst"),
            Ok(CompressionFormat::Zstd)
        ));
        assert!(matches!(
            CompressionFormat::from_url("file:///path/to/snapshot.tar.lz4"),
            Ok(CompressionFormat::Lz4)
        ));
        assert!(matches!(
            CompressionFormat::from_url("file:///path/to/snapshot.tar.zst"),
            Ok(CompressionFormat::Zstd)
        ));
        assert!(CompressionFormat::from_url("https://example.com/snapshot.tar.gz").is_err());
    }
}
