use reqwest::{blocking::Client, header::RANGE};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::mpsc::{channel, Sender},
};
use tracing::debug;

use super::error::DownloaderError;

/// Responses sent by a worker.
#[derive(Debug)]
pub(crate) enum WorkerResponse {
    /// Worker has been spawned and awaiting work.
    Ready { worker_id: u64, tx: Sender<WorkerRequest> },
    /// Worker has downloaded
    DownloadedChunk { worker_id: u64, chunk_index: usize, written_bytes: usize },
    /// Worker has encountered an error.
    Err { worker_id: u64, error: DownloaderError },
}

/// Requests sent to a worker.
#[derive(Debug)]
pub(crate) enum WorkerRequest {
    /// Requests a range to be downloaded.
    Download { chunk_index: usize, start: usize, end: usize },
    /// Signals a worker exit.
    Finish,
}

/// Downloads requested chunk ranges from to the data file.
pub(crate) fn worker_fetch(
    worker_id: u64,
    orchestrator_tx: &Sender<WorkerResponse>,
    data_file: PathBuf,
    url: String,
) -> Result<(), DownloaderError> {
    let client = Client::new();
    let mut data_file = BufWriter::new(OpenOptions::new().write(true).open(data_file)?);

    // Signals readiness to download
    let (tx, rx) = channel::<WorkerRequest>();
    let _ = orchestrator_tx.send(WorkerResponse::Ready { worker_id, tx });

    while let Ok(req) = rx.recv() {
        debug!(target: "sync::stages::s3::downloader", worker_id, ?req, "received from orchestrator");

        match req {
            WorkerRequest::Download { chunk_index, start, end } => {
                data_file.seek(SeekFrom::Start(start as u64))?;

                let mut response =
                    client.get(&url).header(RANGE, format!("bytes={}-{}", start, end)).send()?;

                let written_bytes = std::io::copy(response.by_ref(), &mut data_file)? as usize;

                let _ = orchestrator_tx.send(WorkerResponse::DownloadedChunk {
                    worker_id,
                    chunk_index,
                    written_bytes,
                });
            }
            WorkerRequest::Finish => break,
        }
    }

    Ok(())
}
