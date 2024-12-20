use super::{
    error::DownloaderError,
    meta::Metadata,
    worker::{worker_fetch, WorkerRequest, WorkerResponse},
};
use reqwest::{blocking::Client, header::CONTENT_LENGTH};
use std::{collections::HashMap, fs::OpenOptions, io::Read, path::Path, sync::mpsc::channel};
use tracing::debug;

pub(crate) fn fetch(data_file: &Path, url: &str, mut concurrent: u64) -> Result<(), DownloaderError> {

    let mut metadata = metadata(data_file, url)?;
    if metadata.is_done() {
        return Ok(())
    }

    concurrent = concurrent.min(std::thread::available_parallelism()?.get() as u64);

    // Ensure the file is preallocated
    let file = OpenOptions::new().create(true).read(true).write(true).open(data_file)?;
    if file.metadata()?.len() as usize != metadata.total_size {
        debug!(target: "sync::stages::s3::downloader", ?data_file, length = metadata.total_size, "Preallocating space.");
        file.set_len(metadata.total_size as u64)?;
    }

    // Create channels for communication between workers and orchestrator
    let (chunks_tx, chunks_rx) = channel();

    // Initiate workers
    for worker_id in 0..concurrent {
        let chunks_tx = chunks_tx.clone();
        let data_file = data_file.to_path_buf();
        let url = url.to_string();
        std::thread::spawn(move || {
            if let Err(error) = worker_fetch(worker_id, &chunks_tx, data_file, url) {
                let _ = chunks_tx.send(WorkerResponse::Err { worker_id, error });
            }
        });
    }

    // Drop the sender to allow the loop processing chunks_rx to exit once all workers are done.
    drop(chunks_tx);

    let mut workers = HashMap::new();
    let mut missing = metadata.needed_ranges().into_iter();

    // Distribute chunk ranges to workers when they free up
    while let Ok(worker_msg) = chunks_rx.recv() {
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
        match missing.next() {
            Some((chunk_index, remaining_range)) => {
                if let Some((start, end)) = remaining_range {
                    let _ = worker.send(WorkerRequest::Download { chunk_index, start, end });
                }
            }
            None => {
                let _ = worker.send(WorkerRequest::Finish);
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_download() {
        reth_tracing::init_test_tracing();

        let url = "https://link.testfile.org/500MB";
        let file = &PathBuf::from("/tmp/500MB");

        fetch(file, url, 4).unwrap();
    }
}
