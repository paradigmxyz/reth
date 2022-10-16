//! Basic usage of the torrent downloader.

use futures::StreamExt;
use reth_bittorrent::bittorrent::{Bittorrent, BittorrentConfig};
use tempfile::tempdir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let tmp = tempdir()?;

    let (_client, mut handler) = Bittorrent::new(BittorrentConfig::new(tmp.path()));

    let handle = tokio::task::spawn(async move {
        while let Some(event) = handler.next().await {
            dbg!(event);
        }
    });

    handle.await?;

    Ok(())
}
