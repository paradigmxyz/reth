use futures::Stream;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot::Sender as OneshotSender};

/// Provides the API to interact with torrents.
#[derive(Debug, Clone)]
pub struct Bittorrent {
    /// Channel used to communicate with the [`BittorrentHandler`].
    tx: UnboundedSender<BttCommand>,
}

// === impl Bittorrent ===

impl Bittorrent {
    /// Creates a new instance of [`Bittorrent`] and the [`BittorrentHandler`] task.
    pub fn new(_config: BittorrentConfig) -> (Self, BittorrentHandler) {
        todo!()
    }
}

/// The receiver half of the [`Bittorrent`] instance.
///
/// This can create new torrents and is con communicate with the spawned tasks for torrents and the
/// disk manager. It will also bubble up
///
/// This needs to be spawned on another task, so it get process all commands and events
#[must_use = "Handler does nothing unless polled"]
pub struct BittorrentHandler {}

impl Stream for BittorrentHandler {
    type Item = BittorrentEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        todo!()
    }
}

/// All settings used to create a [`Bittorrent`] instance.
#[derive(Debug, Clone)]
pub struct BittorrentConfig {}

/// User facing events.
pub enum BittorrentEvent {
    TorrentCreated,
    TorrentFinished,
}

/// Commands that can be sent from [`Bittorrent`] to the [`BittorrentHandler`]
enum BttCommand {
    /// Create a new torrent.
    CreateTorrent {
        /// Settings for the torrent
        config: (),
        /// Sender half used to send back the new torrent handle.
        tx: OneshotSender<()>,
    },
    /// Command to gracefully shut down all operations.
    Shutdown,
}
