//! A codec that passes through messages, to be used instead of
//![`tokio_util::codec::BytesCodec`].

use bytes::{BufMut, BytesMut};
use std::io;
use tokio_util::codec::{Decoder, Encoder};

/// A Codec that passes through `bytes::Bytes`. It is similar
/// to [`tokio_util::codec::BytesCodec`], but that does not work
/// because it operates on `BytesMut`.
#[derive(Debug, Default)]
pub struct PassthroughCodec;

impl Decoder for PassthroughCodec {
    type Item = bytes::Bytes;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(src.to_owned().freeze()))
    }
}

impl Encoder<bytes::Bytes> for PassthroughCodec {
    type Error = io::Error;

    fn encode(&mut self, data: bytes::Bytes, buf: &mut BytesMut) -> Result<(), Self::Error> {
        buf.reserve(data.len());
        buf.put(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_codec() {
        let msg = bytes::Bytes::from(vec![1, 2, 3]);
        let mut buf = BytesMut::new();
        let mut codec = PassthroughCodec::default();
        codec.encode(msg.clone(), &mut buf).unwrap();
        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, msg);
    }
}
