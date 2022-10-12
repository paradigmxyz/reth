//! Peer sessions.

use crate::{bitfield::BitField, sha1::Sha1Hash, torrent::torrent::TorrentCtx};
use tokio::time::Instant;

/// An established session to a remote peer
pub struct PeerSession {
    /// Current context of this session
    ctx: PeerCtx,
    /// Access to the context of the torrent that's shared by all peers.
    torrent: TorrentCtx,
}

/// Connection start out as chocked and not interested
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCtx {
    /// The peer's bittorrent id
    pub bittorrent_id: Sha1Hash,
    /// What pieces this peers owns or lacks
    pub bitfield: Option<BitField>,
    /// History of send/receive statistics
    pub stats: PeerStats,
    /// How the remote treats requests made by the client.
    pub remote_choke: ChokeType,
    /// Whether the remote is interested in the content
    pub remote_interest: InterestType,
    /// How the client treats requests from the remote
    pub client_choke: ChokeType,
    /// Whether the client is currently interested to download
    pub client_interest: InterestType,
    /// Timestamp for the last received keepalive msg
    pub remote_heartbeat: Instant,
    /// Timestamp for the last sent keepalive msg
    pub client_heartbeat: Instant,
}

impl PeerCtx {
    pub fn new<T: Into<Sha1Hash>>(peer_btt_id: T) -> Self {
        Self {
            bitfield: None,
            bittorrent_id: peer_btt_id.into(),
            stats: Default::default(),
            remote_choke: Default::default(),
            remote_interest: Default::default(),
            client_choke: Default::default(),
            client_interest: Default::default(),
            remote_heartbeat: Instant::now(),
            client_heartbeat: Instant::now(),
        }
    }

    pub fn new_with_bitfield<T: Into<Sha1Hash>>(peer_btt_id: T, piece_field: BitField) -> Self {
        Self {
            bitfield: Some(piece_field),
            bittorrent_id: peer_btt_id.into(),
            stats: Default::default(),
            remote_choke: Default::default(),
            remote_interest: Default::default(),
            client_choke: Default::default(),
            client_interest: Default::default(),
            remote_heartbeat: Instant::now(),
            client_heartbeat: Instant::now(),
        }
    }

    /// Last Heartbeat from the remote is older than 2 minutes
    pub fn is_remote_timeout(&self, timestamp: Instant) -> bool {
        timestamp.duration_since(self.remote_heartbeat).as_secs() > 120
    }

    /// Our last timeout is older than 2 minutes
    pub fn is_client_timeout(&self, timestamp: Instant) -> bool {
        timestamp.duration_since(self.client_heartbeat).as_secs() > 120
    }

    /// Whether this peer has pieces that the other bitfield is lacking.
    pub fn is_having_missing_pieces_for(&self, other: &BitField) -> bool {
        if let Some(bitfield) = &self.bitfield {
            if bitfield.len() != other.len() {
                return false
            }
            for (i, bit) in bitfield.iter().enumerate() {
                if *bit && !*other.get(i).expect("exists; qed") {
                    return true
                }
            }
            return false
        }
        false
    }

    /// Whether the Peer already sent its bitfield
    pub fn has_bitfield(&self) -> bool {
        self.bitfield.is_some()
    }

    /// Whether the piece is owned by the peer
    pub fn has_piece(&self, piece_index: usize) -> Option<bool> {
        if let Some(field) = &self.bitfield {
            field.get(piece_index).as_deref().copied()
        } else {
            None
        }
    }

    /// The remote is interested to download
    pub fn is_remote_interested(&self) -> bool {
        self.remote_interest == InterestType::Interested
    }

    /// The remote is currently not interested to download
    pub fn is_remote_not_interested(&self) -> bool {
        self.remote_interest == InterestType::Interested
    }

    /// The client is currently choked by the remote
    pub fn is_client_choked_on_remote(&self) -> bool {
        self.remote_choke == ChokeType::Choked
    }

    /// The client is currently choked by the remote
    pub fn is_client_unchoked_on_remote(&self) -> bool {
        self.remote_choke == ChokeType::UnChoked
    }

    /// The client is interested to download
    pub fn is_client_interested(&self) -> bool {
        self.client_interest == InterestType::Interested
    }

    /// The client is currently not interested to download
    pub fn is_client_not_interested(&self) -> bool {
        self.client_interest == InterestType::Interested
    }

    /// The remote is currently choked by the client side
    pub fn is_remote_choked_on_client(&self) -> bool {
        self.client_choke == ChokeType::Choked
    }

    /// The client is currently choked by the remote
    pub fn is_remote_unchoked_on_client(&self) -> bool {
        self.client_choke == ChokeType::UnChoked
    }

    /// Whether requests sent by the client are answered by the remote
    ///
    /// A download only is possible if the client is interested and the is not
    /// choked by the remote and the bitfield was sent to the client
    pub fn remote_can_seed(&self) -> bool {
        self.is_client_unchoked_on_remote() && self.is_client_interested() && self.has_bitfield()
    }

    /// Whether requests by the remote are answered by the client
    ///
    /// To send a piece to remote the remote must be interested and not be
    /// choked by the client
    pub fn remote_can_leech(&self) -> bool {
        self.is_remote_unchoked_on_client() && self.is_remote_interested()
    }

    pub fn remote_can_seed_piece(&self, piece_index: usize) -> bool {
        self.remote_can_seed() && self.has_piece(piece_index).unwrap_or_default()
    }

    /// Set the piece at the index to owned.
    /// If no bitfield is available `None` is returned, otherwise if the
    /// requested `piece_index` is in bounds.
    pub fn add_piece(&mut self, piece_index: usize) -> Option<bool> {
        if let Some(ref mut have) = self.bitfield {
            if piece_index < have.len() {
                have.set(piece_index, true);
                Some(true)
            } else {
                Some(false)
            }
        } else {
            None
        }
    }

    /// Sets the Peer's bitfield and returns if there was already one set.
    pub fn set_bitfield(&mut self, bitfield: BitField) -> Option<BitField> {
        self.bitfield.replace(bitfield)
    }

    /// Set the piece at the index to missing.
    /// If no bitfield is available `None` is returned, otherwise a `Ok` if the
    /// requested `piece_index` is in bounds.
    pub fn remove_piece(&mut self, piece_index: usize) -> Option<Result<(), ()>> {
        if let Some(have) = &mut self.bitfield {
            if piece_index < have.len() {
                have.set(piece_index, false);
                Some(Ok(()))
            } else {
                Some(Err(()))
            }
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerStats {
    pub received_good_blocks: usize,
    pub received_bad_blocks: usize,
    pub send_blocks: usize,
}

/// The state of a single `Peer`.
#[derive(Debug, Copy, Clone)]
enum PeerState {
    /// The peer has not yet been contacted.
    ///
    /// This is the starting state for every peer.
    NotContacted,
    /// The iterator is waiting for a result from the peer.
    Waiting(Instant),

    /// A result was not delivered for the peer within the configured timeout.
    ///
    /// The peer is not taken into account for the termination conditions
    /// of the iterator until and unless it responds.
    Unresponsive,

    /// Obtaining a result from the peer has failed.
    ///
    /// This is a final state, reached as a result of a call to `on_failure`.
    Failed,
    /// A successful result from the peer has been delivered.
    ///
    /// This is a final state, reached as a result of a call to `on_success`.
    Succeeded,
}

/// Status of our connection to a node reported by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ChokeType {
    /// remote has chocked the client
    /// When a peer chokes the client, it is a notification that no requests
    /// will be answered until the client is unchoked.
    Choked = 0,
    /// client currently accepts request
    UnChoked = 1,
}

impl Default for ChokeType {
    fn default() -> Self {
        ChokeType::Choked
    }
}

/// Status of our interest to download a target by the BitTorrent protocol.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InterestType {
    /// remote has no interest to download.
    NotInterested = 0,
    /// remote is currently interested to download.
    Interested = 1,
}

impl Default for InterestType {
    fn default() -> Self {
        InterestType::NotInterested
    }
}
