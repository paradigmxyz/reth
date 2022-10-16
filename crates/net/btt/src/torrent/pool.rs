use crate::torrent::TorrentId;
use fnv::FnvHashMap;

/// Container for all active torrents.
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
}

pub struct TorrentHandle {}
