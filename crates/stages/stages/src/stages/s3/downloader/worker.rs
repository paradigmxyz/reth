use super::error::DownloaderError;
use reqwest::{header::RANGE, Client};
use std::path::{Path, PathBuf};
use tokio::{
    fs::OpenOptions,
    io::{AsyncSeekExt, AsyncWriteExt, BufWriter},
    sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
};
use tracing::debug;

/// Responses sent by a worker.
#[derive(Debug)]
pub(crate) enum WorkerResponse {
    /// Worker has been spawned and awaiting work.
    Ready { worker_id: u64, tx: UnboundedSender<WorkerRequest> },
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

/// Spawns the requested number of workers and returns a `UnboundedReceiver` that all of them will
/// respond to.
pub(crate) fn spawn_workers(
    url: &str,
    worker_count: u64,
    data_file: &Path,
) -> UnboundedReceiver<WorkerResponse> {
    // Create channels for communication between workers and orchestrator
    let (orchestrator_tx, orchestrator_rx) = unbounded_channel();

    // Initiate workers
    for worker_id in 0..worker_count {
        let orchestrator_tx = orchestrator_tx.clone();
        let data_file = data_file.to_path_buf();
        let url = url.to_string();
        debug!(target: "sync::stages::s3::downloader", ?worker_id, "Spawning.");

        tokio::spawn(async move {
            if let Err(error) = worker_fetch(worker_id, &orchestrator_tx, data_file, url).await {
                let _ = orchestrator_tx.send(WorkerResponse::Err { worker_id, error });
            }
        });
    }

    orchestrator_rx
}

/// Downloads requested chunk ranges to the data file.
async fn worker_fetch(
    worker_id: u64,
    orchestrator_tx: &UnboundedSender<WorkerResponse>,
    data_file: PathBuf,
    url: String,
) -> Result<(), DownloaderError> {
    let client = Client::new();
    let mut data_file = BufWriter::new(OpenOptions::new().write(true).open(data_file).await?);

    // Signals readiness to download
    let (tx, mut rx) = unbounded_channel::<WorkerRequest>();
    orchestrator_tx.send(WorkerResponse::Ready { worker_id, tx }).unwrap_or_else(|_| {
        debug!("Failed to notify orchestrator of readiness");
    });

    while let Some(req) = rx.recv().await {
        debug!(
            target: "sync::stages::s3::downloader",
            worker_id,
            ?req,
            "received from orchestrator"
        );

        match req {
            WorkerRequest::Download { chunk_index, start, end } => {
                data_file.seek(tokio::io::SeekFrom::Start(start as u64)).await?;

                let mut response = client
                    .get(&url)
                    .header(RANGE, format!("bytes={}-{}", start, end))
                    .send()
                    .await?;

                let mut written_bytes = 0;
                while let Some(chunk) = response.chunk().await? {
                    written_bytes += chunk.len();
                    data_file.write_all(&chunk).await?;
                }
                data_file.flush().await?;

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
