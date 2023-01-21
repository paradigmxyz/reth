
//!
use std::sync::Arc;
use tokio::sync::mpsc;
use reth_rpc_types::Header;

/// A [Downloader] that drives a spawned [Downloader]
pub struct TaskDownloader {
    from_downloader: mpsc::UnboundedReceiver<Vec<Header>>,
    to_downloader: mpsc::UnboundedSender<DownloaderCommand>
}


/// Commands delegated tot the spawned [Downloader]
enum DownloaderCommand {

}