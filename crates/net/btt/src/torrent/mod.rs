//! Torrent related implementations.

use crate::{
    bitfield::BitField,
    info::{PieceIndex, StorageInfo},
    peer::{PeerEvent, SessionTick},
    sha1::{PeerId, Sha1Hash},
    torrent::{config::TorrentConfig, piece_picker::PiecePicker, tracker::TrackerSession},
    tracker::Tracker,
};
use std::{
    collections::HashMap,
    fmt,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    RwLock,
};

pub(crate) mod config;
pub(crate) mod piece_picker;
pub(crate) mod pool;
pub(crate) mod tracker;

/// Each torrent gets a randomly assigned ID that is globally unique.
/// This id is used in engine APIs to interact with torrents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct TorrentId(pub(crate) u32);

impl fmt::Display for TorrentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t#{}", self.0)
    }
}

/// The channel for communicating with torrent.
pub(crate) type CommandSender = UnboundedSender<Command>;

/// The type of channel on which a torrent can listen for block write
/// completions.
pub(crate) type CommandReceiver = UnboundedReceiver<Command>;

/// The types of messages that the torrent can receive from other parts of the
/// engine.
#[derive(Debug)]
pub(crate) enum Command {
    // /// Sent when some blocks were written to disk or an error occurred while
    // /// writing.
    // PieceCompletion(Result<PieceCompletion, WriteError>),
    /// There was an error reading a block.
    ReadError {
        // block_info: BlockInfo,
        // error: ReadError,
    },
    /// A message sent only once, after the peer has been connected.
    PeerConnected { addr: SocketAddr, id: PeerId },
    /// Peer sessions periodically send this message when they have a state
    /// change.
    PeerState { addr: SocketAddr, info: SessionTick },
    /// Gracefully shut down the torrent.
    ///
    /// This command tells all active peer sessions of torrent to do the same,
    /// waits for them and announces to trackers our exit.
    Shutdown,
}

/// The type returned on completing a piece.
#[derive(Debug)]
pub(crate) struct PieceCompletion {
    /// The index of the piece.
    pub index: PieceIndex,
    /// Whether the piece is valid. If it's not, it's not written to disk.
    pub is_valid: bool,
}

/// Information and methods shared with peer sessions in the torrent.
///
/// This type contains fields that need to be read or updated by peer sessions.
/// Fields expected to be mutated are thus secured for inter-task access with
/// various synchronization primitives.
pub(crate) struct TorrentContext {
    /// The torrent ID, unique in this engine.
    pub id: TorrentId,
    /// The info hash of the torrent, derived from its metainfo. This is used to
    /// identify the torrent with other peers and trackers.
    pub info_hash: Sha1Hash,
    /// The arbitrary client id, chosen by the user of this library. This is
    /// advertised to peers and trackers.
    pub client_id: PeerId,
    /// A copy of the torrent channel sender. This is not used by torrent itself,
    /// but by the peer session tasks to which an arc copy of this torrent
    /// context is given.
    pub cmd_tx: CommandSender,
    /// The piece picker picks the next most optimal piece to download and is
    /// shared by all peers in a torrent.
    pub piece_picker: Arc<RwLock<PiecePicker>>,
    // /// These are the active piece downloads in which the peer sessions in this
    // /// torrent are participating.
    // ///
    // /// They are stored and synchronized in this object to download a piece from
    // /// multiple peers, which helps us to have fewer incomplete pieces.
    // ///
    // /// Peer sessions may be run on different threads, any of which may read and
    // /// write to this map and to the pieces in the map. Thus we need a read
    // /// write lock on both.
    // // TODO: Benchmark whether using the nested locking approach isn't too slow.
    // // For mvp it should do.
    // pub downloads: RwLock<HashMap<PieceIndex, RwLock<PieceDownload>>>,

    // /// The handle to the disk IO task, used to issue commands on it. A copy of
    // /// this handle is passed down to each peer session.
    // pub disk_tx: disk::Sender,
    /// Info about the torrent's storage (piece length, download length, etc).
    pub storage: StorageInfo,
}

/// Parameters for the torrent constructor.
pub(crate) struct Params {
    pub id: TorrentId,
    // pub disk_tx: disk::Sender,
    pub info_hash: Sha1Hash,
    pub storage_info: StorageInfo,
    pub own_pieces: BitField,
    pub trackers: Vec<Tracker>,
    pub client_id: PeerId,
    pub listen_addr: SocketAddr,
    pub conf: TorrentConfig,
    // pub alert_tx: AlertSender,
}

/// Represents a torrent upload or download.
///
/// This is the main entity responsible for the high-level management of
/// a torrent download or upload. It starts and stops connections with peers
/// ([`PeerSession`](crate::peer::PeerSession) instances) and stores metadata
/// about the torrent.
pub(crate) struct Torrent {
    /// The peer session for this torrent
    peers: HashMap<SocketAddr, UnboundedReceiver<PeerEvent>>,
    /// The peers returned by tracker to which we can connect.
    available_peers: Vec<SocketAddr>,
    /// Information that is shared with peer sessions.
    ctx: Arc<TorrentContext>,
    /// Listener for commands.
    cmd_rx: CommandReceiver,
    /// The trackers we can announce to.
    trackers: Vec<TrackerSession>,
    /// The address on which torrent should listen for new peers.
    listen_addr: SocketAddr,
    /// The time the torrent was first started.
    start_time: Option<Instant>,
    /// The total time the torrent has been running.
    ///
    /// This is a separate field as `Instant::now() - start_time` cannot be
    /// relied upon due to the fact that it is possible to pause a torrent, in
    /// which case we don't want to record the run time.
    // TODO: pausing a torrent is not actually at this point, but this is done
    // in expectation of that feature
    run_duration: Duration,
    /// In the last part of the download the torrent is in what's called the
    /// endgame. This is the stage when all pieces have been picked but not all
    /// have been received. There is a tendency for a piece to be mostly
    /// downloaded by one peer, but when only a few pieces are left to complete
    /// the torrent this could defer completion because some of these last
    /// pieces may end up with slower peers.  So when endgame is active, we let
    /// all peers finish the remaining pieces and cancel pending requests from
    /// the slower peers.
    in_endgame: bool,
    // /// Measures various transfer statistics.
    // counters: ThruputCounters,
    /// The configuration of this particular torrent.
    conf: TorrentConfig,
    /// If `TorrentAlertConf::latest_completed_pieces` alert type is set, each
    /// round the torrent collects the pieces that were downloaded, sends them
    /// to peer as an alert, and resets the list.
    ///
    /// This is set to some if the configuration is enabled, and set to none if
    /// disabled.
    completed_pieces: Option<Vec<PieceIndex>>,
}
