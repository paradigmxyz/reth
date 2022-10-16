//! A codec for rlp encoded messages.

use bytes::{Buf, BytesMut};
use reth_rlp::{Decodable, Encodable, Header};
use std::{io, marker::PhantomData};
use tokio_util::codec::{Decoder, Encoder};

/// A Codec that can decodes rlp encoded messages.
#[derive(Debug)]
pub struct RlpCodec<T> {
    read_header: bool,
    payload_length: usize,
    type_handles_header: bool,
    /// The message to encode
    _marker: PhantomData<T>,
}

// === impl RlpCodec ===

impl<T> RlpCodec<T> {
    /// Prepend the `Header` before decoding the target type and also encode the `Header` in this
    /// codec.
    pub fn prepend_header() -> Self {
        Self { type_handles_header: false, ..Default::default() }
    }
}

impl<T> Default for RlpCodec<T> {
    fn default() -> Self {
        Self {
            read_header: false,
            payload_length: 0,
            type_handles_header: true,
            _marker: Default::default(),
        }
    }
}

impl<T: Decodable> Decoder for RlpCodec<T> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }

        if !self.read_header {
            let header = Header::decode(&mut src.as_ref())
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            self.read_header = true;
            if self.type_handles_header {
                self.payload_length = header.payload_length + header.length();
            } else {
                src.advance(header.length());
                self.payload_length = header.payload_length;
            }
        }

        if src.len() < self.payload_length {
            return Ok(None)
        }

        let payload = src.split_to(self.payload_length);
        self.read_header = false;

        let msg = T::decode(&mut payload.as_ref())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        Ok(Some(msg))
    }
}

impl<T: Encodable> Encoder<T> for RlpCodec<T> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        if !self.type_handles_header {
            let mut b = BytesMut::new();
            item.encode(&mut b);
            let payload_length = b.len();
            let header = Header { list: true, payload_length };
            header.encode(dst);
            dst.extend_from_slice(b.as_ref());
        }

        item.encode(dst);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::RequestPair;
    use reth_primitives::H256;

    #[test]
    fn test_header_len() {
        let mut bytes = BytesMut::new();
        let header = Header { list: false, payload_length: 100 };
        header.encode(&mut bytes);
        assert_eq!(bytes.len(), 2);

        let mut bytes = BytesMut::new();
        let header = Header { list: true, payload_length: 100 };
        header.encode(&mut bytes);
        assert_eq!(bytes.len(), 2);
    }

    #[test]
    fn test_rlp_codec() {
        let mut msg = RequestPair { request_id: 99, message: vec![H256::random()] };
        let mut buf = BytesMut::new();
        let mut codec = RlpCodec::default();
        codec.encode(msg.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, msg);

        msg.request_id = 1337;
        let mut buf = BytesMut::new();
        let mut codec = RlpCodec::default();
        codec.encode(msg.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, msg);
    }
}
