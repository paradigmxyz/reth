//! This contains the main codec for `RLPx` ECIES messages

use crate::{algorithm::ECIES, ECIESError, ECIESErrorImpl, EgressECIESValue, IngressECIESValue};
use alloy_primitives::{bytes::BytesMut, B512 as PeerId};
use secp256k1::SecretKey;
use std::{fmt::Debug, io};
use tokio_util::codec::{Decoder, Encoder};
use tracing::{instrument, trace};

/// The max size that the initial handshake packet can be. Currently 2KiB.
const MAX_INITIAL_HANDSHAKE_SIZE: usize = 2048;

/// The largest frame body the 24-bit size field of an `RLPx` frame header can describe.
pub(crate) const MAX_FRAME_SIZE: usize = (1 << 24) - 1;

/// The largest buffer allocation a connection keeps after a frame is read or flushed.
///
/// Normal gossip stays below it: transaction broadcasts are soft-capped at 128KiB, and the write
/// buffer holds at most one frame past the 64KiB
/// [`DEFAULT_BACKPRESSURE_BOUNDARY`](crate::stream::DEFAULT_BACKPRESSURE_BOUNDARY). Larger frames,
/// such as block bodies or pooled transactions responses of up to 2MiB, grow the buffer only until
/// they are consumed, so an idle connection does not keep their capacity.
pub(crate) const MAX_RETAINED_BUFFER_CAPACITY: usize = 256 * 1024;

/// The capacity of a buffer that replaces a released one, the same as the initial capacity
/// tokio-util's `Framed` gives its read and write buffers.
pub(crate) const INITIAL_BUFFER_CAPACITY: usize = 8 * 1024;

/// Tokio codec for ECIES
#[derive(Debug)]
pub struct ECIESCodec {
    ecies: ECIES,
    state: ECIESState,
}

/// Current ECIES state of a connection
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ECIESState {
    /// The first stage of the ECIES handshake, where each side of the connection sends an auth
    /// message containing the ephemeral public key, signature of the public key, nonce, and other
    /// metadata.
    Auth,

    /// The second stage of the ECIES handshake, where each side of the connection sends an ack
    /// message containing the nonce and other metadata.
    Ack,

    /// This is the same as the [`ECIESState::Header`] stage, but occurs only after the first
    /// [`ECIESState::Ack`] message. This is so that the initial handshake message can be properly
    /// validated.
    InitialHeader,

    /// The third stage of the ECIES handshake, where header is parsed, message integrity checks
    /// performed, and message is decrypted.
    Header,

    /// The final stage, where the ECIES message is actually read and returned by the ECIES codec.
    Body,
}

impl ECIESCodec {
    /// Create a new server codec using the given secret key
    pub(crate) fn new_server(secret_key: SecretKey) -> Result<Self, ECIESError> {
        Ok(Self { ecies: ECIES::new_server(secret_key)?, state: ECIESState::Auth })
    }

    /// Create a new client codec using the given secret key and the server's public id
    pub(crate) fn new_client(secret_key: SecretKey, remote_id: PeerId) -> Result<Self, ECIESError> {
        Ok(Self { ecies: ECIES::new_client(secret_key, remote_id)?, state: ECIESState::Auth })
    }
}

impl Decoder for ECIESCodec {
    type Item = IngressECIESValue;
    type Error = ECIESError;

    #[instrument(level = "trace", target = "net::ecies", skip_all, fields(peer=?self.ecies.remote_id, state=?self.state))]
    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.state {
                ECIESState::Auth => {
                    trace!("parsing auth");
                    if buf.len() < 2 {
                        return Ok(None)
                    }

                    let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    let total_size = payload_size + 2;

                    if buf.len() < total_size {
                        trace!("current len {}, need {}", buf.len(), total_size);
                        return Ok(None)
                    }

                    self.ecies.read_auth(&mut buf.split_to(total_size))?;

                    self.state = ECIESState::InitialHeader;
                    return Ok(Some(IngressECIESValue::AuthReceive(self.ecies.remote_id())))
                }
                ECIESState::Ack => {
                    trace!("parsing ack with len {}", buf.len());
                    if buf.len() < 2 {
                        return Ok(None)
                    }

                    let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                    let total_size = payload_size + 2;

                    if buf.len() < total_size {
                        trace!("current len {}, need {}", buf.len(), total_size);
                        return Ok(None)
                    }

                    self.ecies.read_ack(&mut buf.split_to(total_size))?;

                    self.state = ECIESState::InitialHeader;
                    return Ok(Some(IngressECIESValue::Ack))
                }
                ECIESState::InitialHeader => {
                    if buf.len() < ECIES::header_len() {
                        trace!("current len {}, need {}", buf.len(), ECIES::header_len());
                        return Ok(None)
                    }

                    let body_size =
                        self.ecies.read_header(&mut buf.split_to(ECIES::header_len()))?;

                    if body_size > MAX_INITIAL_HANDSHAKE_SIZE {
                        trace!(?body_size, max=?MAX_INITIAL_HANDSHAKE_SIZE, "Body exceeds max initial handshake size");
                        return Err(ECIESErrorImpl::InitialHeaderBodyTooLarge {
                            body_size,
                            max_body_size: MAX_INITIAL_HANDSHAKE_SIZE,
                        }
                        .into())
                    }

                    self.state = ECIESState::Body;
                }
                ECIESState::Header => {
                    if buf.len() < ECIES::header_len() {
                        trace!("current len {}, need {}", buf.len(), ECIES::header_len());
                        return Ok(None)
                    }

                    self.ecies.read_header(&mut buf.split_to(ECIES::header_len()))?;

                    self.state = ECIESState::Body;
                }
                ECIESState::Body => {
                    let body_len = self.ecies.body_len();
                    // The buffer grows only as the body arrives. The body length comes from the
                    // peer, so reserving it up front would let a peer make the node allocate up
                    // to `MAX_FRAME_SIZE` for a header alone.
                    if buf.len() < body_len {
                        return Ok(None)
                    }

                    let mut data = buf.split_to(body_len);
                    if body_len > MAX_RETAINED_BUFFER_CAPACITY {
                        // `buf` still shares the frame's allocation and reclaims all of it on its
                        // next `reserve` after `data` is dropped. Move the bytes of the next
                        // frame, if any, to a new buffer, so the allocation is freed with `data`.
                        let mut next =
                            BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY.max(buf.len()));
                        next.extend_from_slice(buf);
                        *buf = next;
                    }
                    let body_len = {
                        let body = self.ecies.read_body(&mut data)?;
                        body.len()
                    };
                    data.truncate(body_len);

                    self.state = ECIESState::Header;
                    return Ok(Some(IngressECIESValue::Message(data)))
                }
            }
        }
    }
}

impl Encoder<EgressECIESValue> for ECIESCodec {
    type Error = io::Error;

    #[instrument(level = "trace", target = "net::ecies", skip(self, buf), fields(peer=?self.ecies.remote_id, state=?self.state))]
    fn encode(&mut self, item: EgressECIESValue, buf: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            EgressECIESValue::Auth => {
                self.state = ECIESState::Ack;
                self.ecies.write_auth(buf);
            }
            EgressECIESValue::Ack => {
                self.state = ECIESState::InitialHeader;
                self.ecies.write_ack(buf);
            }
            EgressECIESValue::Message(data) => {
                if data.len() > MAX_FRAME_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        ECIESError::from(ECIESErrorImpl::FrameTooLarge {
                            size: data.len(),
                            max_size: MAX_FRAME_SIZE,
                        }),
                    ))
                }
                self.ecies.write_header(buf, data.len());
                self.ecies.write_body(buf, &data);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::bytes::Bytes;
    use reth_network_peers::pk2id;

    /// Returns a client and server codec that completed the handshake.
    fn codec_pair() -> (ECIESCodec, ECIESCodec) {
        let server_key = SecretKey::new(&mut rand::rng());
        let mut server = ECIESCodec::new_server(server_key).unwrap();
        let mut client = ECIESCodec::new_client(
            SecretKey::new(&mut rand::rng()),
            pk2id(&server_key.public_key()),
        )
        .unwrap();

        let mut wire = BytesMut::new();
        client.encode(EgressECIESValue::Auth, &mut wire).unwrap();
        assert!(matches!(
            server.decode(&mut wire).unwrap(),
            Some(IngressECIESValue::AuthReceive(_))
        ));
        server.encode(EgressECIESValue::Ack, &mut wire).unwrap();
        assert_eq!(client.decode(&mut wire).unwrap(), Some(IngressECIESValue::Ack));

        // The first frame after the handshake is limited to `MAX_INITIAL_HANDSHAKE_SIZE`.
        client.encode(EgressECIESValue::Message(Bytes::from_static(b"hello")), &mut wire).unwrap();
        assert_eq!(
            server.decode(&mut wire).unwrap(),
            Some(IngressECIESValue::Message(BytesMut::from(&b"hello"[..])))
        );
        assert!(wire.is_empty());
        (client, server)
    }

    /// Encodes each payload as a frame and returns the frames' bytes.
    fn encode_frames(codec: &mut ECIESCodec, payloads: &[Vec<u8>]) -> BytesMut {
        let mut wire = BytesMut::new();
        for payload in payloads {
            codec.encode(EgressECIESValue::Message(payload.clone().into()), &mut wire).unwrap();
        }
        wire
    }

    /// Feeds `wire` to the codec the way tokio-util's `Framed` does: decodes while frames are
    /// available, then reserves one byte and reads up to `read_size` bytes into the spare
    /// capacity. Frames are dropped once copied out, as the `p2p` layer does after decompressing
    /// them. Returns the payloads and the largest capacity the buffer reached.
    fn read_frames(
        codec: &mut ECIESCodec,
        buf: &mut BytesMut,
        mut wire: &[u8],
        read_size: usize,
    ) -> (Vec<Vec<u8>>, usize) {
        let mut payloads = Vec::new();
        let mut peak_capacity = buf.capacity();
        loop {
            while let Some(value) = codec.decode(buf).unwrap() {
                let IngressECIESValue::Message(frame) = value else {
                    panic!("unexpected {value:?}")
                };
                payloads.push(frame.to_vec());
            }
            if wire.is_empty() {
                return (payloads, peak_capacity)
            }
            buf.reserve(1);
            peak_capacity = peak_capacity.max(buf.capacity());
            let n = read_size.min(buf.capacity() - buf.len()).min(wire.len());
            buf.extend_from_slice(&wire[..n]);
            wire = &wire[n..];
        }
    }

    fn payload(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_add(seed)).collect()
    }

    #[test]
    fn read_buffer_released_after_large_frame() {
        let (mut client, mut server) = codec_pair();
        let large = 2 << 20;
        let payloads = [payload(large, 1), payload(200, 2), payload(1000, 3), payload(300, 4)];
        let wire = encode_frames(&mut client, &payloads);

        let mut buf = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);
        let (read, peak_capacity) = read_frames(&mut server, &mut buf, &wire, 64 * 1024);
        assert_eq!(read, payloads);

        // The buffer grows only by doubling as the body arrives.
        assert!(peak_capacity <= 2 * (large + 32));
        // The buffer cannot reclaim more than the retain limit without allocating.
        assert!(buf.is_empty());
        assert!(!buf.try_reclaim(MAX_RETAINED_BUFFER_CAPACITY + 1));
    }

    #[test]
    fn header_alone_does_not_allocate_the_body() {
        let (mut client, mut server) = codec_pair();
        let wire = encode_frames(&mut client, &[payload(8 << 20, 5)]);

        // Feed the header and a few bytes of an 8MiB body.
        let mut buf = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);
        buf.extend_from_slice(&wire[..64]);
        assert_eq!(server.decode(&mut buf).unwrap(), None);
        assert!(buf.capacity() <= INITIAL_BUFFER_CAPACITY);
    }

    #[test]
    fn small_frames_do_not_reallocate_read_buffer() {
        let (mut client, mut server) = codec_pair();
        let mut buf = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);
        assert!(buf.try_reclaim(INITIAL_BUFFER_CAPACITY));
        let start = buf.as_ptr() as usize;
        let end = start + buf.capacity();

        for round in 0..200u8 {
            let payloads: Vec<_> =
                (0..8).map(|i| payload(64 + 97 * usize::from(i + round % 7), round ^ i)).collect();
            let wire = encode_frames(&mut client, &payloads);
            let (read, peak_capacity) = read_frames(&mut server, &mut buf, &wire, 1500);
            assert_eq!(read, payloads);
            assert!(peak_capacity <= INITIAL_BUFFER_CAPACITY);
            let ptr = buf.as_ptr() as usize;
            assert!((start..=end).contains(&ptr), "read buffer moved to a new allocation");
        }
    }

    #[test]
    fn decodes_partial_and_back_to_back_frames() {
        let payloads = [
            payload(1, 1),
            payload(300, 2),
            payload(MAX_RETAINED_BUFFER_CAPACITY + 1, 3),
            payload(16, 4),
            payload(5000, 5),
            payload(1 << 20, 6),
            payload(17, 7),
        ];
        for read_size in [1, 7, 33, 4096, 64 * 1024, usize::MAX] {
            let (mut client, mut server) = codec_pair();
            let wire = encode_frames(&mut client, &payloads);
            let mut buf = BytesMut::with_capacity(INITIAL_BUFFER_CAPACITY);
            let (read, _) = read_frames(&mut server, &mut buf, &wire, read_size);
            assert_eq!(read, payloads, "read size {read_size}");
            assert!(buf.is_empty());
        }

        // All frames already in the buffer, as after one large read.
        let (mut client, mut server) = codec_pair();
        let mut buf = encode_frames(&mut client, &payloads);
        for expected in &payloads {
            let Some(IngressECIESValue::Message(frame)) = server.decode(&mut buf).unwrap() else {
                panic!("expected a frame")
            };
            assert_eq!(&frame[..], &expected[..]);
        }
        assert_eq!(server.decode(&mut buf).unwrap(), None);
        assert!(buf.is_empty());
    }

    #[test]
    fn keeps_next_frame_bytes_after_large_frame() {
        let (mut client, mut server) = codec_pair();
        let large = payload(2 << 20, 1);
        let small = payload(100, 2);
        let mut buf = encode_frames(&mut client, std::slice::from_ref(&large));
        let next = encode_frames(&mut client, std::slice::from_ref(&small));
        buf.extend_from_slice(&next[..10]);

        let Some(IngressECIESValue::Message(frame)) = server.decode(&mut buf).unwrap() else {
            panic!("expected a frame")
        };
        assert_eq!(&frame[..], &large[..]);
        drop(frame);
        assert_eq!(&buf[..], &next[..10]);
        assert!(!buf.try_reclaim(MAX_RETAINED_BUFFER_CAPACITY + 1));

        assert_eq!(server.decode(&mut buf).unwrap(), None);
        buf.extend_from_slice(&next[10..]);
        assert_eq!(
            server.decode(&mut buf).unwrap(),
            Some(IngressECIESValue::Message(small[..].into()))
        );
        assert!(buf.is_empty());
    }
}
