use crate::torrent::{Torrent, TorrentId};
use fnv::FnvHashMap;
use futures::{channel::mpsc, Stream};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Container for _all_ active torrents.
pub(crate) struct TorrentPool {
    /// Identifier counter for new torrents.
    id: u32,
    /// All active torrents.
    torrents: FnvHashMap<TorrentId, TorrentHandle>,
}

// === impl TorrentPool ===

impl TorrentPool {
    /// Returns a new torrent identifier
    fn next_id(&mut self) -> TorrentId {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        TorrentId(id)
    }

    pub(crate) fn start_torrent(&mut self) {}
}

impl Stream for TorrentPool {
    type Item = TorrentEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // poll all torrents
        todo!()
    }
}

/// Events produced by individual torrents.
#[derive(Debug)]
pub enum TorrentEvent {}

pub struct TorrentHandle {
    /// Receiver half for events for this torrent.
    rx: mpsc::Receiver<TorrentEvent>,
}
