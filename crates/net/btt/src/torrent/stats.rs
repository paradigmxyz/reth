use crate::{
    counter::{ChannelCounter, Counter, ThruputCounters},
    info::PieceIndex,
    peer::state::SessionState,
    sha1::PeerId,
};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

/// Aggregated statistics of a torrent.
#[derive(Clone, Debug, Default)]
pub struct TorrentStats {
    /// When the torrent was _first_ started.
    pub start_time: Option<Instant>,
    /// How long the torrent has been running.
    pub run_duration: Duration,
    /// Aggregate statistics about a torrent's pieces.
    pub pieces: PieceStats,
    /// The peers of the torrent.
    ///
    /// By default, only the number of connected peers are sent with each
    /// torrent tick. This is the most efficient option.
    ///
    /// However, if enabled in the torrent's configuration, a full list of peers
    /// with aggregate statistics is sent with each tick.
    pub peers: Peers,
    /// Various thruput statistics of the torrent.
    pub thruput: ThruputStats,
}

/// Statistics of a torrent's pieces.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PieceStats {
    /// The total number of pieces in torrent.
    pub total: usize,
    /// The number of pieces that the torrent is currently downloading.
    pub pending: usize,
    /// The number of pieces that the torrent has downloaded.
    pub complete: usize,
    /// The pieces that were completed since the last tick.
    ///
    /// By default this information is not sent, as it has some overhead. It
    /// needs to be turned on in the torrent's [configuration]
    /// (crate::conf::TorrentAlertConf::completed_pieces).
    pub latest_completed: Option<Vec<PieceIndex>>,
}

impl PieceStats {
    /// Returns whether the torrent is a seed.
    pub fn is_seed(&self) -> bool {
        self.complete == self.total
    }

    /// Returns whether the torrent is in endgame mode (about to finish
    /// download).
    pub fn is_in_endgame(&self) -> bool {
        self.pending + self.complete == self.total
    }
}

/// Limited or full information of a torrent's peer sessions.
#[derive(Clone, Debug)]
pub enum Peers {
    /// The number of connected peers.
    Count(usize),
    /// The full list of connected peers, with aggregate statistics for each.
    Full(Vec<PeerSessionStats>),
}

impl Peers {
    /// Returns the number of connected peers.
    pub fn len(&self) -> usize {
        match self {
            Self::Count(n) => *n,
            Self::Full(peers) => peers.len(),
        }
    }

    /// Returns true when there are no connected peers in torrent.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Peers {
    fn default() -> Self {
        Self::Count(0)
    }
}

/// Aggregate statistics of a peer session.
#[derive(Clone, Debug)]
pub struct PeerSessionStats {
    /// The IP-port pair of the peer.
    pub addr: SocketAddr,
    /// Peer's 20 byte BitTorrent id. Updated when the peer sends us its peer
    /// id, in the handshake.
    pub id: Option<PeerId>,
    /// The current state of the session.
    pub state: SessionState,
    /// The number of pieces the peer has.
    pub piece_count: usize,
    /// Various thruput statistics of ths peer.
    pub thruput: ThruputStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThruputStats {
    /// Statistics about the protocol transfer rates in both directions.
    pub protocol: Channel,
    /// Statistics about the payload transfer rates in both directions.
    pub payload: Channel,
    /// If a peer has sent us data that we already have, it is recorded here as
    /// waste.
    pub waste: u64,
}

impl From<&ThruputCounters> for ThruputStats {
    fn from(c: &ThruputCounters) -> Self {
        Self {
            protocol: Channel::from(&c.protocol),
            payload: Channel::from(&c.payload),
            waste: c.waste.round(),
        }
    }
}

/// Aggregate statistics about a communication channel, e.g. protocol chatter
/// or exchanged payload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Channel {
    pub down: Thruput,
    pub up: Thruput,
}

impl From<&ChannelCounter> for Channel {
    fn from(c: &ChannelCounter) -> Self {
        Self { down: Thruput::from(&c.down), up: Thruput::from(&c.up) }
    }
}

/// Statistics of a torrent's current thruput.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Thruput {
    pub total: u64,
    pub rate: u64,
    pub peak: u64,
}

impl From<&Counter> for Thruput {
    fn from(c: &Counter) -> Self {
        Self { total: c.total(), rate: c.avg(), peak: c.peak() }
    }
}
