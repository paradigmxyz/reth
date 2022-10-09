//! Torrent support

use crate::sha1::PeerId;
use std::collections::HashMap;
use tokio::sync::mpsc::{Receiver, Sender};

/// Manages a single torrent.
pub(crate) struct Torrent {
    /// Active peer sessions.
    peers: HashMap<PeerId, ()>,
    /// receiver half that listens for commands.
    rx: Receiver<TorrentCommand>,
    /// Contains status of this torrent
    ctx: TorrentCtx,
}

/// A handle to a spawned [`Torrent`].
#[derive(Debug, Clone)]
pub(crate) struct TorrentHandle {
    /// sender half to communicate with a [`Torrent`] task
    tx: Sender<TorrentCommand>,
}

/// Container for the context of a [`Torrent`]
pub(crate) struct TorrentCtx {}

/// Commands that can be sent to the [`Torrent`] task.
enum TorrentCommand {}

/// Unique identifier for an active Torrent operation.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub(crate) struct TorrentId(pub(crate) u64);

/// Identifies a peer with the internal id and the protocol id.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct TorrentPeer {
    /// Protocol Identifier for this peer.
    pub(crate) peer_id: PeerId,
    /// the bittorrent id, necessary to for handshaking
    pub(crate) torrent_id: TorrentId,
}
