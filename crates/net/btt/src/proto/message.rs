// //! All bittorrent message types.

use crate::{bitfield::BitField, piece::Piece, sha1::Sha1Hash};
use bytes::{BufMut, BytesMut};

/// A request for a specific (part) of a piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRequest {
    /// specifying the zero-based piece index
    pub index: u32,
    /// specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// specifying the requested length.
    pub length: u32,
}

/// All the remaining messages in the protocol take the form of <length
/// prefix><message ID><payload>. The length prefix is a four byte big-endian
/// value. The message ID is a single decimal byte. integers in the peer wire
/// protocol are encoded as four byte big-endian values
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum PeerMessage {
    /// heartbeat generally 2 minute interval
    KeepAlive,
    Choke,
    UnChoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield {
        /// bitfield representing the pieces that have been successfully
        /// downloaded
        index_field: BitField,
    },
    Request {
        request: PeerRequest,
    },
    Cancel {
        request: PeerRequest,
    },
    Piece {
        piece: Piece,
    },
    Handshake {
        handshake: Handshake,
    },
    /// he port message is sent by newer versions of the Mainline that
    /// implements a DHT tracker. The listen port is the port this peer's
    /// DHT node is listening on. This peer should be inserted in the local
    /// routing table (if DHT tracker is supported).
    Port {
        port: u16,
    },
}

impl PeerMessage {
    /// Message identifier for `CHOKE`
    pub const CHOKE_ID: u8 = 0;
    /// Message identifier for `UNCHOKE`
    pub const UNCHOKE_ID: u8 = 1;
    /// Message identifier for `INTERESTED`
    pub const INTERESTED_ID: u8 = 2;
    /// Message identifier for `NOT INTERESTED`
    pub const NOT_INTERESTED_ID: u8 = 3;
    /// Message identifier for `HAVE`
    pub const HAVE_ID: u8 = 4;
    /// Message identifier for `BITFIELD`
    pub const BITFIELD_ID: u8 = 5;
    /// Message identifier for `REQUEST`
    pub const REQUEST_ID: u8 = 6;
    /// Message identifier for `PIECE`
    pub const PIECE_ID: u8 = 7;
    /// Message identifier for `CANCEL`
    pub const CANCEL_ID: u8 = 8;
    /// Message identifier for `PORT`
    pub const PORT_ID: u8 = 9;
    /// Message identifier for `HANDSHAKE`, the length of the handshake
    pub const HANDSHAKE_ID: u8 = 19;
    /// Message identifier for `KEEP_ALIVE`
    pub const KEEP_ALIVE_ID: [u8; 4] = [0, 0, 0, 0];

    /// Returns the length in bytes that should be reserved for serialising the message.
    ///
    /// Besides `PeerMessage::Handshake` all messages are prefixed by its length
    /// (4 byte big endian). Besides `PeerMessage::KeepAlive` and
    /// `PeerMessage::Handshake` every message is identified by single decimal
    /// byte id
    pub fn size(&self) -> usize {
        4 + match self {
            PeerMessage::KeepAlive => 0,
            PeerMessage::Choke |
            PeerMessage::UnChoke |
            PeerMessage::Interested |
            PeerMessage::NotInterested => 1,
            PeerMessage::Have { .. } => 1 + 4,
            PeerMessage::Bitfield { index_field } => {
                let mut len = index_field.len() / 8;
                let rem = index_field.len() % 8;
                if rem > 0 {
                    len += 1;
                }
                1 + len
            }
            PeerMessage::Request { request: _peer_request } |
            PeerMessage::Cancel { request: _peer_request } => 1 + 12,
            PeerMessage::Piece { piece } => 1 + 8 + piece.data.len(),
            PeerMessage::Handshake { .. } => 45 + 19,
            PeerMessage::Port { .. } => 1 + 4,
        }
    }

    /// Returns the header length of the specific message.
    pub fn header_len(&self) -> u64 {
        match self {
            PeerMessage::Choke => 4 + 1,
            PeerMessage::UnChoke => 4 + 1,
            PeerMessage::Interested => 4 + 1,
            PeerMessage::NotInterested => 4 + 1,
            PeerMessage::Have { .. } => 4 + 1 + 4,
            PeerMessage::Bitfield { .. } => 4 + 1,
            PeerMessage::Request { .. } => 4 + 1 + 3 * 4,
            PeerMessage::Piece { .. } => 4 + 1 + 2 * 4,
            PeerMessage::Cancel { .. } => 4 + 1 + 3 * 4,
            PeerMessage::KeepAlive => 1,
            _ => 0,
        }
    }

    /// Encodes the message into the given buf.
    pub fn encode_into(&self, buf: &mut BytesMut) {
        match self {
            PeerMessage::KeepAlive => buf.put_u32(0),
            PeerMessage::Choke => {
                buf.put_u32(1);
                buf.put_u8(0);
            }
            PeerMessage::UnChoke => {
                buf.put_u32(1);
                buf.put_u8(1);
            }
            PeerMessage::Interested => {
                buf.put_u32(1);
                buf.put_u8(2);
            }
            PeerMessage::NotInterested => {
                buf.put_u32(1);
                buf.put_u8(3);
            }
            PeerMessage::Have { index } => {
                buf.put_u32(5);
                buf.put_u8(4);
                buf.put_u32(*index);
            }
            PeerMessage::Bitfield { index_field } => {
                // 1 byte message id and n byte f bitfield
                //
                // NOTE: `bitfield.len()` returns the number of _bits_
                let msg_len = 1 + index_field.len() / 8;
                buf.put_u32(msg_len as u32);
                buf.put_u8(5);
                // sparse bits are zero
                buf.extend_from_slice(index_field.as_raw_slice());
            }
            PeerMessage::Request { request: peer_request } => {
                buf.put_u32(13);
                buf.put_u8(6);
                buf.put_u32(peer_request.index);
                buf.put_u32(peer_request.begin);
                buf.put_u32(peer_request.length);
            }
            PeerMessage::Piece { piece } => {
                buf.put_u32(9 + piece.data.len() as u32);
                buf.put_u8(7);
                buf.put_u32(piece.index);
                buf.put_u32(piece.begin);
                buf.extend_from_slice(&piece.data);
            }
            PeerMessage::Cancel { request: peer_request } => {
                buf.put_u32(13);
                buf.put_u8(8);
                buf.put_u32(peer_request.index);
                buf.put_u32(peer_request.begin);
                buf.put_u32(peer_request.length);
            }
            PeerMessage::Port { port } => {
                buf.put_u32(3);
                buf.put_u8(9);
                buf.put_u16(*port);
            }
            PeerMessage::Handshake { handshake } => {
                buf.extend_from_slice(b"\x13");
                buf.extend_from_slice(&Handshake::BITTORRENT_IDENTIFIER);
                buf.extend_from_slice(&handshake.reserved);
                buf.extend_from_slice(handshake.info_hash.as_ref());
                buf.extend_from_slice(handshake.peer_id.as_ref());
            }
        }
    }
}

/// The handshake is a required message and must be the first message
/// transmitted by the client. `handshake:
/// `<pstrlen><pstr><reserved><info_hash><peer_id>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    /// eight (8) reserved bytes. All current implementations use all zeroes.
    /// Each bit in these bytes can be used to change the behavior of the
    /// protocol. An email from Bram suggests that trailing bits should be
    /// used first, so that leading bits may be used to change the meaning of
    /// trailing bits.
    pub reserved: [u8; 8],
    /// 20-byte SHA1 hash of the info key in the metainfo file.
    /// This is the same info_hash that is transmitted in tracker requests.
    pub info_hash: Sha1Hash,
    /// 20-byte string used as a unique ID for the client
    pub peer_id: Sha1Hash,
}

impl Handshake {
    /// Bittorrent protocol identifier.
    pub const BITTORRENT_IDENTIFIER_STR: &'static str = "BitTorrent protocol";

    /// `BITTORRENT_IDENTIFIER` as byte array.
    pub const BITTORRENT_IDENTIFIER: [u8; 19] =
        [66, 105, 116, 84, 111, 114, 114, 101, 110, 116, 32, 112, 114, 111, 116, 111, 99, 111, 108];

    /// Create a new `Handshake` message
    pub fn new(info_hash: Sha1Hash, peer_id: Sha1Hash) -> Self {
        Self { reserved: [0; 8], info_hash, peer_id }
    }

    /// Create a new `Handshake` message with a random peer id.
    pub fn new_with_random_id(info_hash: Sha1Hash) -> Self {
        Self { reserved: [0u8; 8], info_hash, peer_id: Sha1Hash::random() }
    }
}
