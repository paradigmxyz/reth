//! Bittorrent codec implementation

use crate::{
    bitfield::BitField,
    piece::{Piece, BLOCK_SIZE_MAX},
    proto::message::{Handshake, PeerMessage, PeerRequest},
};
use byteorder::{BigEndian, ByteOrder};
use bytes::{Buf, Bytes, BytesMut};
use std::io::{self};
use tokio_util::codec::{Decoder, Encoder};

/// Bittorrent codec for Messages sent over the wire.
#[derive(Clone, Debug)]
pub struct PeerCodec {
    /// maximum permitted number of bytes per frame
    max_length: usize,
}

impl PeerCodec {
    /// Create a new codec with max allowed buffer length.
    pub fn new_with_max_length(max_length: usize) -> Self {
        Self { max_length }
    }
}

impl Default for PeerCodec {
    fn default() -> Self {
        Self {
            // length(4) + identifier(1) + payload (index(4) + begin(4) + max allowed blocksize)
            max_length: 4 + 1 + 8 + BLOCK_SIZE_MAX,
        }
    }
}

impl Decoder for PeerCodec {
    type Item = PeerMessage;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            return Ok(None)
        }
        if src[0] == PeerMessage::HANDSHAKE_ID {
            // handshake
            if src.len() < 68 {
                src.reserve(68 - src.len());
                return Ok(None)
            }
            if src[1..20] != Handshake::BITTORRENT_IDENTIFIER {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported Handshake Protocol identifier",
                ))
            }
            let mut msg = src.split_to(68);
            msg.advance(20);
            Ok(Some(PeerMessage::Handshake {
                handshake: Handshake {
                    reserved: msg[..8].try_into().unwrap(),
                    info_hash: msg[8..28].try_into().unwrap(),
                    peer_id: msg[28..48].try_into().unwrap(),
                },
            }))
        } else {
            // read length
            match BigEndian::read_u32(&src[..4]) {
                0 => {
                    src.advance(4);
                    Ok(Some(PeerMessage::KeepAlive))
                }
                1 => {
                    if src.len() < 5 {
                        return Ok(None)
                    }

                    let msg = match src[4] {
                        PeerMessage::CHOKE_ID => Ok(Some(PeerMessage::Choke)),
                        PeerMessage::UNCHOKE_ID => Ok(Some(PeerMessage::UnChoke)),
                        PeerMessage::INTERESTED_ID => Ok(Some(PeerMessage::Interested)),
                        PeerMessage::NOT_INTERESTED_ID => Ok(Some(PeerMessage::NotInterested)),
                        i => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Unexpected Peer Message with length 1 and id {}", i),
                            ))
                        }
                    };
                    src.advance(5);
                    msg
                }
                5 => {
                    if src.len() < 9 {
                        src.reserve(9 - src.len());
                        return Ok(None)
                    }
                    if src[4] == PeerMessage::HAVE_ID {
                        let msg = src.split_to(9);
                        Ok(Some(PeerMessage::Have { index: BigEndian::read_u32(&msg[5..9]) }))
                    } else {
                        Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Unexpected Peer Message with length 5 and id {}", src[4]),
                        ))
                    }
                }
                msg_len => {
                    let msg_len = msg_len as usize;

                    if msg_len > self.max_length {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Message to large"))
                    }

                    let total_len = 4 + msg_len;
                    if src.remaining() < total_len {
                        // we don't have a full message in the buffer
                        src.reserve(total_len - src.remaining());
                        return Ok(None)
                    }

                    if src[4] == PeerMessage::BITFIELD_ID {
                        // in the `BitField` msg the length is the amount of bits
                        let number_of_pieces = msg_len - 1;
                        if number_of_pieces == 0 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "Empty BitField message not supported",
                            ))
                        }

                        src.advance(5);
                        let msg = src.split_to(number_of_pieces);
                        let index_field = BitField::from_bytes(&msg);

                        return Ok(Some(PeerMessage::Bitfield { index_field }))
                    }

                    let msg_bytes_length = 4 + msg_len;
                    if src.len() < msg_bytes_length {
                        src.reserve(msg_bytes_length - src.len());
                        return Ok(None)
                    }

                    match src[4] {
                        PeerMessage::REQUEST_ID => {
                            let msg = src.split_to(msg_bytes_length);
                            Ok(Some(PeerMessage::Request {
                                request: PeerRequest {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    length: BigEndian::read_u32(&msg[13..17]),
                                },
                            }))
                        }
                        PeerMessage::PIECE_ID => {
                            let msg = src.split_to(msg_bytes_length);
                            Ok(Some(PeerMessage::Piece {
                                piece: Piece {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    data: Bytes::from(msg[13..].to_vec()),
                                },
                            }))
                        }
                        PeerMessage::CANCEL_ID => {
                            let msg = src.split_to(msg_bytes_length);
                            Ok(Some(PeerMessage::Cancel {
                                request: PeerRequest {
                                    index: BigEndian::read_u32(&msg[5..9]),
                                    begin: BigEndian::read_u32(&msg[9..13]),
                                    length: BigEndian::read_u32(&msg[13..17]),
                                },
                            }))
                        } // [11001001, 10000011, 11111011
                        PeerMessage::PORT_ID => {
                            let msg = src.split_to(msg_bytes_length);
                            Ok(Some(PeerMessage::Port { port: BigEndian::read_u16(&msg[5..]) }))
                        }
                        i => Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("Unexpected Peer Message with length 1 and id {}", i),
                        )),
                    }
                }
            }
        }
    }
}

impl Encoder<PeerMessage> for PeerCodec {
    type Error = io::Error;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        dst.reserve(item.size());
        item.encode_into(dst);
        Ok(())
    }
}

// tests adapted from <https://github.com/mandreyel/cratetorrent/blob/34aa13835872a14f00d4a334483afff79181999f/cratetorrent/src/peer/codec.rs#L517>
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bitfield::BitField, piece::BLOCK_SIZE_MIN, sha1::Sha1Hash};
    use bytes::BufMut;

    macro_rules! peer_wire_msg_encode_decode {
        ($( $msg:expr ),*) => {
            let mut len = 0;
            $(
                len += $msg.size();
            )*
            let mut buf = BytesMut::with_capacity(len);
            let mut codec = PeerCodec::default();

            $(
              codec.encode($msg.clone(), &mut buf).unwrap();
            )*

            $(
              assert_eq!(Some($msg), codec.decode(&mut buf).unwrap());
            )*
        };
    }

    #[test]
    fn peer_wire_codec() {
        let handshake =
            PeerMessage::Handshake { handshake: Handshake::new_with_random_id(Sha1Hash::random()) };

        peer_wire_msg_encode_decode!(
            PeerMessage::KeepAlive,
            PeerMessage::Choke,
            PeerMessage::UnChoke,
            PeerMessage::Interested,
            PeerMessage::NotInterested,
            PeerMessage::Have { index: 100 },
            PeerMessage::Bitfield { index_field: BitField::from_bytes(&[0b10100000, 0b00010010]) },
            PeerMessage::Request { request: PeerRequest { index: 1, begin: 2, length: 16384 } },
            PeerMessage::Piece {
                piece: Piece {
                    index: 1,
                    begin: 2,
                    data: std::iter::repeat(1).take(16384).collect()
                }
            },
            PeerMessage::Cancel { request: PeerRequest { index: 1, begin: 2, length: 16384 } },
            PeerMessage::Port { port: 8080 },
            handshake
        );
    }

    /// Tests a stream of arbitrary messages to ensure that not only do they
    /// encode and then decode correctly (like the individual test cases
    /// ascertain), but that the buffer cursor is properly advanced by the codec
    /// implementation in both cases.
    #[test]
    fn test_message_stream() {
        let (handshake, encoded_handshake) = make_handshake();
        let msgs = [
            make_choke(),
            make_unchoke(),
            make_keep_alive(),
            make_interested(),
            make_not_interested(),
            make_bitfield(),
            make_have(),
            make_request(),
            make_piece(),
            make_piece(),
            make_keep_alive(),
            make_interested(),
            make_cancel(),
            make_piece(),
            make_not_interested(),
            make_choke(),
            make_choke(),
        ];

        // create byte stream of all above messages
        let msgs_len = msgs.iter().fold(0, |acc, (_, encoded)| acc + encoded.len());
        let mut read_buf = BytesMut::with_capacity(msgs_len);
        read_buf.extend_from_slice(&encoded_handshake);
        for (_, encoded) in &msgs {
            read_buf.extend_from_slice(encoded);
        }

        // decode messages one by one from the byte stream in the same order as
        // they were encoded, starting with the handshake
        let decoded_handshake = PeerCodec::default().decode(&mut read_buf).unwrap();
        assert_eq!(decoded_handshake, Some(PeerMessage::Handshake { handshake }));
        for (msg, _) in &msgs {
            let decoded_msg = PeerCodec::default().decode(&mut read_buf).unwrap();
            assert_eq!(decoded_msg.unwrap(), *msg);
        }
    }

    // This test attempts to simulate a closer to real world use case than
    // `test_test_message_stream`, by progresively loading up the codec's read
    // buffer with the encoded message bytes, asserting that messages are
    // decoded correctly even if their bytes arrives in different chunks.
    //
    // This is a regression test in that there used to be a bug that failed to
    // parse block messages (the largest message type) if the full message
    // couldn't be received (as is often the case).
    #[test]
    fn test_chunked_message_stream() {
        let mut read_buf = BytesMut::new();

        // start with the handshake by adding only the first half of it to the
        // buffer
        let (handshake, encoded_handshake) = make_handshake();
        let handshake_split_pos = encoded_handshake.len() / 2;
        read_buf.extend_from_slice(&encoded_handshake[0..handshake_split_pos]);

        // can't decode the handshake without the full message
        assert!(PeerCodec::default().decode(&mut read_buf).unwrap().is_none());

        // the handshake should successfully decode with the second half added
        read_buf.extend_from_slice(&encoded_handshake[handshake_split_pos..]);
        let decoded_handshake = PeerCodec::default().decode(&mut read_buf).unwrap();
        assert_eq!(decoded_handshake, Some(PeerMessage::Handshake { handshake }));

        let msgs = [
            make_choke(),
            make_unchoke(),
            make_interested(),
            make_not_interested(),
            make_bitfield(),
            make_have(),
            make_request(),
            make_piece(),
            make_piece(),
            make_interested(),
            make_cancel(),
            make_piece(),
            make_not_interested(),
            make_choke(),
            make_choke(),
        ];

        // go through all above messages and do the same procedure as with the
        // handshake: add the first half, fail to decode, add the second half,
        // decode successfully
        for (msg, encoded) in &msgs {
            // add the first half of the message
            let split_pos = encoded.len() / 2;
            read_buf.extend_from_slice(&encoded[0..split_pos]);
            // fail to decode
            assert!(PeerCodec::default().decode(&mut read_buf).unwrap().is_none());
            // add the second half
            read_buf.extend_from_slice(&encoded[split_pos..]);
            let decoded_msg = PeerCodec::default().decode(&mut read_buf).unwrap();
            assert_eq!(decoded_msg.unwrap(), *msg);
        }
    }

    /// Tests that the decoding of various invalid handshake messages results in
    /// an error.
    #[test]
    fn test_invalid_handshake_decoding() {
        // try to decode a handshake with an invalid protocol string
        let mut invalid_encoded = {
            let prot = "not the BitTorrent protocol";
            // these buffer values don't matter here as we're only expecting
            // invalid encodings
            let reserved = [0; 8];
            let info_hash = [0; 20];
            let peer_id = [0; 20];

            let buf_len = prot.len() + 49;
            let mut buf = BytesMut::with_capacity(buf_len);
            // the message length prefix is not actually included in the value
            let prot_len = prot.len() as u8;
            buf.put_u8(prot_len);
            buf.extend_from_slice(prot.as_bytes());
            buf.extend_from_slice(&reserved);
            buf.extend_from_slice(&info_hash);
            buf.extend_from_slice(&peer_id);
            buf
        };
        let result = PeerCodec::default().decode(&mut invalid_encoded);
        assert!(result.is_err());
    }

    // Returns a `Handshake` and its expected encoded variant.
    fn make_handshake() -> (Handshake, Bytes) {
        // the reserved field is all zeros for now as we don't use extensions
        // yet so we're not testing it
        let reserved = [0; 8];

        // this is not a valid info hash but it doesn't matter for the purposes
        // of this test
        const INFO_HASH: &str = "da39a3ee5e6b4b0d3255";
        let mut info_hash = [0; 20];
        info_hash.copy_from_slice(INFO_HASH.as_bytes());

        const PEER_ID: &str = "cbt-2020-03-03-00000";
        let mut peer_id = [0; 20];
        peer_id.copy_from_slice(PEER_ID.as_bytes());

        let handshake =
            Handshake { reserved, info_hash: info_hash.into(), peer_id: peer_id.into() };

        // than building up result (as that's what the encoder does too and we
        // need to test that it does it correctly)
        let encoded = {
            let buf_len = 68;
            let mut buf = Vec::with_capacity(buf_len);
            // the message length prefix is not actually included in the value
            let prot_len = Handshake::BITTORRENT_IDENTIFIER.len() as u8;
            buf.push(prot_len);
            buf.extend_from_slice(&Handshake::BITTORRENT_IDENTIFIER);
            buf.extend_from_slice(&reserved);
            buf.extend_from_slice(&info_hash);
            buf.extend_from_slice(&peer_id);
            buf
        };

        (handshake, encoded.into())
    }

    /// Tests the encoding and subsequent decoding of a valid 'choke' message.
    #[test]
    fn test_keep_alive_codec() {
        let (msg, expected_encoded) = make_keep_alive();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'choke' message.
    #[test]
    fn test_choke_codec() {
        let (msg, expected_encoded) = make_choke();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'unchoke' message.
    #[test]
    fn test_unchoke_codec() {
        let (msg, expected_encoded) = make_unchoke();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'interested'
    /// message.
    #[test]
    fn test_interested_codec() {
        let (msg, expected_encoded) = make_interested();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'not interested'
    /// message.
    #[test]
    fn test_not_interested_codec() {
        let (msg, expected_encoded) = make_not_interested();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'bitfield' message.
    #[test]
    fn test_bitfield_codec() {
        let (msg, expected_encoded) = make_bitfield();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'have' message.
    #[test]
    fn test_have_codec() {
        let (msg, expected_encoded) = make_have();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'request' message.
    #[test]
    fn test_request_codec() {
        let (msg, expected_encoded) = make_request();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'block' message.
    #[test]
    fn test_block_codec() {
        let (msg, expected_encoded) = make_piece();
        assert_message_codec(msg, expected_encoded);
    }

    /// Tests the encoding and subsequent decoding of a valid 'cancel' message.
    #[test]
    fn test_cancel_codec() {
        let (msg, expected_encoded) = make_cancel();
        assert_message_codec(msg, expected_encoded);
    }

    /// Helper function that asserts that a message is encoded and subsequently
    /// decoded correctly.
    fn assert_message_codec(msg: PeerMessage, expected_encoded: Bytes) {
        // encode message
        let mut encoded = BytesMut::with_capacity(expected_encoded.len());
        PeerCodec::default().encode(msg.clone(), &mut encoded).unwrap();
        assert_eq!(encoded, expected_encoded);

        // don't decode message if there aren't enough bytes in source buffer
        let mut partial_encoded = encoded[0..encoded.len() - 1].into();
        let decoded = PeerCodec::default().decode(&mut partial_encoded).unwrap();
        assert_eq!(decoded, None);

        // decode same message
        let decoded = PeerCodec::default().decode(&mut encoded).unwrap();
        assert_eq!(decoded, Some(msg));
    }

    fn make_keep_alive() -> (PeerMessage, Bytes) {
        (PeerMessage::KeepAlive, Bytes::from_static(&[0; 4]))
    }

    // Returns `Choke` and its expected encoded variant.
    fn make_choke() -> (PeerMessage, Bytes) {
        (PeerMessage::Choke, make_empty_msg_encoded_payload(PeerMessage::CHOKE_ID))
    }

    /// Returns `Unchoke` and its expected encoded variant.
    fn make_unchoke() -> (PeerMessage, Bytes) {
        (PeerMessage::UnChoke, make_empty_msg_encoded_payload(PeerMessage::UNCHOKE_ID))
    }

    /// Returns `Interested` and its expected encoded variant.
    fn make_interested() -> (PeerMessage, Bytes) {
        (PeerMessage::Interested, make_empty_msg_encoded_payload(PeerMessage::INTERESTED_ID))
    }

    /// Returns `NotInterested` and its expected encoded variant.
    fn make_not_interested() -> (PeerMessage, Bytes) {
        (PeerMessage::NotInterested, make_empty_msg_encoded_payload(PeerMessage::NOT_INTERESTED_ID))
    }

    /// Helper used to create 'choke', 'unchoke', 'interested', and 'not
    /// interested' encoded messages that all have the same format.
    fn make_empty_msg_encoded_payload(id: u8) -> Bytes {
        // 1 byte message id
        let msg_len = 1;
        // 4 byte message length prefix and message length
        let buf_len = 4 + msg_len as usize;
        let mut buf = BytesMut::with_capacity(buf_len);
        buf.put_u32(msg_len);
        buf.put_u8(id);
        buf.into()
    }

    /// Returns `Bitfield` and its expected encoded variant.
    fn make_bitfield() -> (PeerMessage, Bytes) {
        let bitfield = BitField::from_vec(vec![0b11001001, 0b10000011, 0b11111011]);
        let encoded = {
            // 1 byte message id and n byte f bitfield
            //
            // NOTE: `bitfield.len()` returns the number of _bits_
            let msg_len = 1 + bitfield.len() / 8;
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(PeerMessage::BITFIELD_ID);
            buf.extend_from_slice(bitfield.as_raw_slice());
            buf
        };
        let msg = PeerMessage::Bitfield { index_field: bitfield };
        (msg, encoded.into())
    }

    /// Returns `Have` and its expected encoded variant.
    fn make_have() -> (PeerMessage, Bytes) {
        let index = 42;
        let msg = PeerMessage::Have { index };
        let encoded = {
            // 1 byte message id and 4 byte piece index
            let msg_len = 1 + 4;
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(PeerMessage::HAVE_ID);
            // ok to unwrap, only used in tests
            buf.put_u32(index);
            buf
        };
        (msg, encoded.into())
    }

    /// Returns `Request` and its expected encoded variant.
    fn make_request() -> (PeerMessage, Bytes) {
        let index = 42u32;
        let begin = 0x4000;
        let length = BLOCK_SIZE_MIN as u32;
        let msg = PeerMessage::Request { request: PeerRequest { index, begin, length } };
        let encoded =
            make_block_info_encoded_msg_payload(PeerMessage::REQUEST_ID, index, begin, length);
        (msg, encoded)
    }

    /// Returns `Piece` and its expected encoded variant.
    fn make_piece() -> (PeerMessage, Bytes) {
        let index = 42;
        let begin = 0x4000;
        let data = vec![0; 0x4000];
        // TODO: fill the block with random values
        let encoded = {
            // 1 byte message id, 4 byte piece index, 4 byte begin, and n byte
            // block
            let msg_len = 1 + 4 + 4 + data.len();
            // 4 byte message length prefix and message length
            let buf_len = 4 + msg_len;
            let mut buf = BytesMut::with_capacity(buf_len);
            buf.put_u32(msg_len as u32);
            buf.put_u8(PeerMessage::PIECE_ID);
            buf.put_u32(index);
            buf.put_u32(begin);
            buf.extend_from_slice(&data);
            buf
        };
        let msg = PeerMessage::Piece { piece: Piece { index, begin, data: data.into() } };
        (msg, encoded.into())
    }

    /// Returns `Cancel` and its expected encoded variant.
    fn make_cancel() -> (PeerMessage, Bytes) {
        let index = 42;
        let begin = 0x4000;
        let length = BLOCK_SIZE_MIN as u32;
        let msg = PeerMessage::Cancel { request: PeerRequest { index, begin, length } };

        let encoded =
            make_block_info_encoded_msg_payload(PeerMessage::CANCEL_ID, index, begin, length);
        (msg, encoded)
    }

    /// Helper used to create 'request' and 'cancel' encoded messages that have
    /// the same format.
    fn make_block_info_encoded_msg_payload(id: u8, index: u32, begin: u32, len: u32) -> Bytes {
        // 1 byte message id, 4 byte piece index, 4 byte begin, 4 byte
        // length
        let msg_len = 1 + 4 + 4 + 4;
        // 4 byte message length prefix and message length
        let buf_len = 4 + msg_len as usize;
        let mut buf = BytesMut::with_capacity(buf_len);
        buf.put_u32(msg_len);
        buf.put_u8(id);
        buf.put_u32(index);
        buf.put_u32(begin);
        buf.put_u32(len);
        buf.into()
    }
}
