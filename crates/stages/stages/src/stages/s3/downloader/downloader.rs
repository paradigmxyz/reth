use super::{
    error::DownloaderError,
    meta::Metadata,
    worker::{worker_fetch, WorkerRequest, WorkerResponse},
};
use alloy_primitives::B256;
use reqwest::{blocking::Client, header::CONTENT_LENGTH};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufReader, Read},
    path::Path,
    sync::mpsc::channel,
};
use tracing::debug;

/// Downloads file from url to data file path.
///
/// If a `file_hash` is passed, it will verify it at the end.
pub(crate) fn fetch(
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
    let mut metadata = metadata(&data_file, url)?;
    if metadata.is_done() {
        return Ok(())
    }

    // Ensure the file is preallocated
    {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&data_file)?;
        if file.metadata()?.len() as usize != metadata.total_size {
            debug!(target: "sync::stages::s3::downloader", ?data_file, length = metadata.total_size, "Preallocating space.");
            file.set_len(metadata.total_size as u64)?;
        }
    }

    while !metadata.is_done() {
        // Find the missing file chunks and the minimum number of workers required
        let missing_chunks = metadata.needed_ranges();
        concurrent = concurrent
            .min(std::thread::available_parallelism()?.get() as u64)
            .min(missing_chunks.len() as u64);

        // Create channels for communication between workers and orchestrator
        let (orchestrator_tx, orchestrator_rx) = channel();

        // Initiate workers
        for worker_id in 0..concurrent {
            let orchestrator_tx = orchestrator_tx.clone();
            let data_file = data_file.clone();
            let url = url.to_string();
            std::thread::spawn(move || {
                if let Err(error) = worker_fetch(worker_id, &orchestrator_tx, data_file, url) {
                    let _ = orchestrator_tx.send(WorkerResponse::Err { worker_id, error });
                }
            });
        }

        // Drop the sender to allow the loop processing to exit once all workers are done
        drop(orchestrator_tx);

        let mut workers = HashMap::new();
        let mut missing_chunks = missing_chunks.into_iter();

        // Distribute chunk ranges to workers when they free up
        while let Ok(worker_msg) = orchestrator_rx.recv() {
            debug!(target: "sync::stages::s3::downloader", ?worker_msg, "received message from worker");

            let available_worker = match worker_msg {
                WorkerResponse::Ready { worker_id, tx } => {
                    workers.insert(worker_id, tx);
                    worker_id
                }
                WorkerResponse::DownloadedChunk { worker_id, chunk_index, written_bytes } => {
                    metadata.update_chunk(chunk_index, written_bytes)?;
                    worker_id
                }
                WorkerResponse::Err { error, .. } => return Err(error),
            };

            let worker = workers.get(&available_worker).expect("should exist");
            match missing_chunks.next() {
                Some((chunk_index, (start, end))) => {
                    let _ = worker.send(WorkerRequest::Download { chunk_index, start, end });
                }
                None => {
                    let _ = worker.send(WorkerRequest::Finish);
                }
            }
        }
    }

    if let Some(file_hash) = file_hash {
        check_file_hash(&data_file, &file_hash)?;
    }

    // Move downloaded file to desired directory.
    metadata.delete()?;
    reth_fs_util::rename(data_file, target_dir.join(filename))?;

    Ok(())
}

/// Creates a metadata file used to keep track of the downloaded chunks. Useful on resuming after a
/// shutdown.
fn metadata(data_file: &Path, url: &str) -> Result<Metadata, DownloaderError> {
    if Metadata::file_path(data_file).exists() {
        debug!(target: "sync::stages::s3::downloader", ?data_file, "Loading metadata ");
        return Metadata::load(data_file)
    }

    let client = Client::new();
    let resp = client.head(url).send()?;
    let total_length: usize = resp
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or("No content length")
        .unwrap();

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

    #[test]
    fn test_download() {
        reth_tracing::init_test_tracing();

        let b3sum = b256!("81a7318f69fc1d6bb0a58a24af302f3b978bc75a435e4ae5d075f999cd060cfd");
        let url = "https://link.testfile.org/500MB";

        let file = tempfile::NamedTempFile::new().unwrap();
        let filename = file.path().file_name().unwrap().to_str().unwrap();
        let target_dir = file.path().parent().unwrap();
        fetch(filename, target_dir, url, 4, Some(b3sum)).unwrap();
    }
}
