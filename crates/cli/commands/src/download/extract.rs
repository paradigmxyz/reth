use super::{
    fetch::{ArchiveFetcher, DownloadedArchive},
    progress::{
        ArchiveExtractionProgress, ArchiveExtractionProgressHandle, DownloadProgress,
        DownloadRequestLimiter, ProgressReader, SharedProgress, SharedProgressReader,
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
    cancel_token: CancellationToken,
) -> Result<()> {
    let progress_reader = ProgressReader::new(reader, total_size, cancel_token);

    match format {
        CompressionFormat::Lz4 => {
            let decoder = Decoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
        }
        CompressionFormat::Zstd => {
            let decoder = ZstdDecoder::new(progress_reader)?;
            Archive::new(decoder).unpack(target_dir)?;
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
    progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    match format {
        CompressionFormat::Lz4 => {
            unpack_archive(Archive::new(Decoder::new(reader)?), target_dir, progress)?;
        }
        CompressionFormat::Zstd => {
            unpack_archive(Archive::new(ZstdDecoder::new(reader)?), target_dir, progress)?;
        }
    }

    Ok(())
}

fn unpack_archive<R: Read>(
    mut archive: Archive<R>,
    target_dir: &Path,
    mut progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    let entries = archive.entries().wrap_err_with(|| {
        format!("failed to read archive entries for `{}`", target_dir.display())
    })?;

    for entry in entries {
        let mut entry = entry.wrap_err_with(|| {
            format!("failed to read archive entry for `{}`", target_dir.display())
        })?;
        extract_entry_with_progress(&mut entry, target_dir, progress.as_deref_mut())?;
    }

    Ok(())
}

fn extract_entry_with_progress<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    target_dir: &Path,
    progress: Option<&mut ArchiveExtractionProgress>,
) -> Result<()> {
    let size = entry.header().entry_size().unwrap_or(0);
    let entry_type = entry.header().entry_type();

    if !entry_type.is_file() || size == 0 {
        entry.unpack_in(target_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    }

    if size < STREAMING_EXTRACTION_PROGRESS_MIN_FILE_SIZE {
        entry.unpack_in(target_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        if let Some(progress) = progress {
            progress.record_extracted(size);
        }
        return Ok(())
    }

    let Some(progress_handle) = progress.as_ref().and_then(|progress| progress.handle()) else {
        entry.unpack_in(target_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    };

    let Some(entry_path) = entry_destination_path(entry, target_dir)? else {
        entry.unpack_in(target_dir).wrap_err_with(|| {
            format!("failed to extract archive into `{}`", target_dir.display())
        })?;
        return Ok(())
    };

    let stop = Arc::new(AtomicBool::new(false));
    let monitor = spawn_extraction_progress_monitor(entry_path, progress_handle, Arc::clone(&stop));
    let unpack_result = entry
        .unpack_in(target_dir)
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
) -> Result<Option<PathBuf>> {
    let mut file_dst = target_dir.to_path_buf();
    let path = entry.path().wrap_err("invalid path in archive entry")?;

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
fn extract_from_file(path: &Path, format: CompressionFormat, target_dir: &Path) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let total_size = file.metadata()?.len();
    info!(target: "reth::cli",
        file = %path.display(),
        size = %DownloadProgress::format_size(total_size),
        "Extracting local archive"
    );
    let start = Instant::now();
    extract_archive(file, total_size, format, target_dir, CancellationToken::new())?;
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
    session: &DownloadSession,
) -> Result<()> {
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
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
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
            extract_archive_raw(reader, format, target_dir, None)
        } else {
            let total_size = response.content_length().unwrap_or(0);
            extract_archive(
                response,
                total_size,
                format,
                target_dir,
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
                    std::thread::sleep(Duration::from_secs(RETRY_BACKOFF_SECS));
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        eyre::eyre!("Streaming download failed after {MAX_DOWNLOAD_RETRIES} attempts")
    }))
}

/// Fetches the snapshot from a remote URL with resume support, then extracts it.
fn download_and_extract(
    url: &str,
    format: CompressionFormat,
    target_dir: &Path,
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
        extract_archive_raw(file, format, target_dir, None)?;
    } else {
        extract_archive(file, total_size, format, target_dir, session.cancel_token().clone())?;
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
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    request_limiter: Option<Arc<DownloadRequestLimiter>>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let format = CompressionFormat::from_url(url)?;

    if let Ok(parsed_url) = Url::parse(url) &&
        parsed_url.scheme() == "file"
    {
        let session = DownloadSession::new(shared, request_limiter, cancel_token);
        let file_path = parsed_url
            .to_file_path()
            .map_err(|_| eyre::eyre!("Invalid file:// URL path: {}", url))?;
        let result = extract_from_file(&file_path, format, target_dir);
        if result.is_ok() {
            session.record_archive_output_complete(file_path.metadata()?.len());
        }
        result
    } else if let Some(request_limiter) = request_limiter {
        download_and_extract(
            url,
            format,
            target_dir,
            DownloadSession::new(shared, Some(request_limiter), cancel_token),
        )
    } else if resumable {
        let session =
            DownloadSession::new(shared, Some(DownloadRequestLimiter::new(1)), cancel_token);
        download_and_extract(url, format, target_dir, session)
    } else {
        let session = DownloadSession::new(shared, None, cancel_token);
        let result = streaming_download_and_extract(url, format, target_dir, &session);
        if result.is_ok() {
            session.record_archive_output_complete(0);
        }
        result
    }
}

/// Downloads and extracts a snapshot archive asynchronously.
///
/// When `shared` is provided, download progress is reported to the shared
/// counter for aggregated display. Otherwise uses a local progress bar.
/// When `resumable` is true, uses two-phase download with `.part` files.
pub(crate) async fn stream_and_extract(
    url: &str,
    target_dir: &Path,
    shared: Option<Arc<SharedProgress>>,
    resumable: bool,
    request_limiter: Option<Arc<DownloadRequestLimiter>>,
    cancel_token: CancellationToken,
) -> Result<()> {
    let target_dir = target_dir.to_path_buf();
    let url = url.to_string();
    task::spawn_blocking(move || {
        blocking_download_and_extract(
            &url,
            &target_dir,
            shared,
            resumable,
            request_limiter,
            cancel_token,
        )
    })
    .await??;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
