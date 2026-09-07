use super::{
    fetch::{ArchiveFetcher, DownloadedArchive},
    progress::{
        ArchiveExtractionProgress, ArchiveExtractionProgressHandle, DownloadProgress,
        DownloadRequestLimiter, ProgressReader, SharedProgressReader,
    },
    session::DownloadSession,
    MAX_DOWNLOAD_RETRIES, RETRY_BACKOFF_SECS,
};
use eyre::{Result, WrapErr};
use lz4::Decoder;
use reqwest::blocking::Client as BlockingClient;
use reth_cli_util::cancellation::CancellationToken;
use reth_fs_util as fs;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tar::Archive;
use tokio::task;
use tracing::{info, warn};
use url::Url;
use zstd::stream::read::Decoder as ZstdDecoder;

const EXTENSION_TAR_LZ4: &str = ".tar.lz4";
const EXTENSION_TAR_ZSTD: &str = ".tar.zst";
const STREAMING_EXTRACTION_PROGRESS_MIN_FILE_SIZE: u64 = 64 * 1024 * 1024;
const EXTRACTION_PROGRESS_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
            unpack_archive(Archive::new(decoder), target_dir, static_files_dir, None)?;
        }
        CompressionFormat::Zstd => {
            let decoder = ZstdDecoder::new(progress_reader)?;
            unpack_archive(Archive::new(decoder), target_dir, static_files_dir, None)?;
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
            )?;
        }
        CompressionFormat::Zstd => {
            unpack_archive(
                Archive::new(ZstdDecoder::new(reader)?),
                target_dir,
                static_files_dir,
                progress,
            )?;
        }
    }

    Ok(())
}

fn unpack_archive<R: Read>(
    mut archive: Archive<R>,
    target_dir: &Path,
    static_files_dir: Option<&Path>,
    mut progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    if static_files_dir.is_none() && progress.is_none() {
        archive.unpack(target_dir)?;
        return Ok(())
    }
    let entries = archive.entries().wrap_err_with(|| {
        format!("failed to read archive entries for `{}`", target_dir.display())
    })?;

    for entry in entries {
        let mut entry = entry.wrap_err_with(|| {
            format!("failed to read archive entry for `{}`", target_dir.display())
        })?;
        extract_entry_with_progress(
            &mut entry,
            target_dir,
            static_files_dir,
            progress.as_deref_mut(),
        )?;
    }

    Ok(())
}

/// Returns the path within the static files directory, accepting tar's optional `./` prefix.
pub(crate) fn static_file_relative_path(path: &Path) -> Option<&Path> {
    path.strip_prefix(".").unwrap_or(path).strip_prefix("static_files").ok()
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
    let mut current = root.clone();
    for part in relative_path.components() {
        current.push(part);
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            eyre::ensure!(
                !metadata.file_type().is_symlink(),
                "Static file archive path contains a symlink"
            );
        }
    }
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
    let DownloadedArchive { path: downloaded_path, size: total_size } = fetcher.download(None)?;

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
    task::spawn_blocking(move || {
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
