use crate::stages::s3::downloader::{worker::spawn_workers, RemainingChunkRange};

use super::{
    error::DownloaderError,
    meta::Metadata,
    worker::{WorkerRequest, WorkerResponse},
};
use alloy_primitives::B256;
use reqwest::{header::CONTENT_LENGTH, Client};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::BufReader,
    path::Path,
};
use tracing::{debug, error, info};

/// Downloads file from url to data file path.
///
/// If a `file_hash` is passed, it will verify it at the end.
///
/// ## Details
///
/// 1) A [`Metadata`] file is created or opened in `{target_dir}/download/{filename}.metadata`. It
///    tracks the download progress including total file size, downloaded bytes, chunk sizes, and
///    ranges that still need downloading. Allows for resumability.
/// 2) The target file is preallocated with the total size of the file in
///    `{target_dir}/download/{filename}`.
/// 3) Multiple `workers` are spawned for downloading of specific chunks of the file.
/// 4) `Orchestrator` manages workers, distributes chunk ranges, and ensures the download progresses
///    efficiently by dynamically assigning tasks to workers as they become available.
/// 5) Once the file is downloaded:
///     * If `file_hash` is `Some`, verifies its blake3 hash.
///     * Deletes the metadata file
///     * Moves downloaded file to target directory.
pub async fn fetch(
    filename: &str,
    target_dir: &Path,
    url: &str,
    mut concurrent: u64,
    file_hash: Option<B256>,
) -> Result<(), DownloaderError> {
    // Create a temporary directory to download files to, before moving them to target_dir.
    let download_dir = target_dir.join("download");
    reth_fs_util::create_dir_all(&download_dir)?;

    let data_file = download_dir.join(filename);
    let mut metadata = metadata(&data_file, url).await?;
    if metadata.is_done() {
        return Ok(())
    }

    // Ensure the file is preallocated so we can download it concurrently
    {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&data_file)?;

        if file.metadata()?.len() as usize != metadata.total_size {
            info!(target: "sync::stages::s3::downloader", ?filename, length = metadata.total_size, "Preallocating space.");
            file.set_len(metadata.total_size as u64)?;
        }
    }

    while !metadata.is_done() {
        info!(target: "sync::stages::s3::downloader", ?filename, "Downloading.");

        // Find the missing file chunks and the minimum number of workers required
        let missing_chunks = metadata.needed_ranges();
        concurrent = concurrent
            .min(std::thread::available_parallelism()?.get() as u64)
            .min(missing_chunks.len() as u64);

        let mut orchestrator_rx = spawn_workers(url, concurrent, &data_file);

        let mut workers = HashMap::new();
        let mut missing_chunks = missing_chunks.into_iter();

        // Distribute chunk ranges to workers when they free up
        while let Some(worker_msg) = orchestrator_rx.recv().await {
            debug!(target: "sync::stages::s3::downloader", ?worker_msg, "received message from worker");

            let available_worker = match worker_msg {
                WorkerResponse::Ready { worker_id, tx } => {
                    debug!(target: "sync::stages::s3::downloader", ?worker_id, "Worker ready.");
                    workers.insert(worker_id, tx);
                    worker_id
                }
                WorkerResponse::DownloadedChunk { worker_id, chunk_index, written_bytes } => {
                    metadata.update_chunk(chunk_index, written_bytes)?;
                    worker_id
                }
                WorkerResponse::Err { worker_id, error } => {
                    error!(target: "sync::stages::s3::downloader", ?worker_id, "Worker found an error: {:?}", error);
                    return Err(error)
                }
            };

            let msg = if let Some(RemainingChunkRange { index, start, end }) = missing_chunks.next()
            {
                debug!(target: "sync::stages::s3::downloader", ?available_worker, start, end, "Worker download request.");
                WorkerRequest::Download { chunk_index: index, start, end }
            } else {
                debug!(target: "sync::stages::s3::downloader", ?available_worker, "Sent Finish command to worker.");
                WorkerRequest::Finish
            };

            let _ = workers.get(&available_worker).expect("should exist").send(msg);
        }
    }

    if let Some(file_hash) = file_hash {
        info!(target: "sync::stages::s3::downloader", ?filename, "Checking file integrity.");
        check_file_hash(&data_file, &file_hash)?;
    }

    // No longer need the metadata file.
    metadata.delete()?;

    // Move downloaded file to desired directory.
    let file_directory = target_dir.join(filename);
    reth_fs_util::rename(data_file, &file_directory)?;
    info!(target: "sync::stages::s3::downloader", ?file_directory, "Moved file from temporary to target directory.");

    Ok(())
}

/// Creates a metadata file used to keep track of the downloaded chunks. Useful on resuming after a
/// shutdown.
async fn metadata(data_file: &Path, url: &str) -> Result<Metadata, DownloaderError> {
    if Metadata::file_path(data_file).exists() {
        debug!(target: "sync::stages::s3::downloader", ?data_file, "Loading metadata ");
        return Metadata::load(data_file)
    }

    let client = Client::new();
    let resp = client.head(url).send().await?;
    let total_length: usize = resp
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or(DownloaderError::EmptyContentLength)?;

    debug!(target: "sync::stages::s3::downloader", ?data_file, "Creating metadata ");

    Metadata::builder(data_file).with_total_size(total_length).build()
}

/// Ensures the file on path has the expected blake3 hash.
fn check_file_hash(path: &Path, expected: &B256) -> Result<(), DownloaderError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut reader, &mut hasher)?;

    let file_hash = hasher.finalize();
    if file_hash.as_bytes() != expected {
        return Err(DownloaderError::InvalidFileHash(file_hash.as_bytes().into(), *expected))
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[tokio::test]
    async fn test_download() {
        reth_tracing::init_test_tracing();

        let b3sum = b256!("81a7318f69fc1d6bb0a58a24af302f3b978bc75a435e4ae5d075f999cd060cfd");
        let url = "https://link.testfile.org/500MB";

        let file = tempfile::NamedTempFile::new().unwrap();
        let filename = file.path().file_name().unwrap().to_str().unwrap();
        let target_dir = file.path().parent().unwrap();
        fetch(filename, target_dir, url, 4, Some(b3sum)).await.unwrap();
    }
}
