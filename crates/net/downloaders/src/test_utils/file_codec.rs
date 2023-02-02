//! Codec for reading raw block bodies from a file.
use super::FileClientError;
use bytes::{Buf, BytesMut};
use reth_eth_wire::RawBlockBody;
use reth_rlp::{Decodable, Encodable};
use tokio_util::codec::{Decoder, Encoder};

/// Codec for reading raw block bodies from a file.
///
/// If using with [`FramedRead`](tokio_util::codec::FramedRead), the user should make sure the
/// framed reader has capacity for the entire block file. Otherwise, the decoder will return
/// [`InputTooShort`](reth_rlp::DecodeError::InputTooShort), because RLP headers can only be
/// decoded if the internal buffer is large enough to contain the entire block body.
///
/// Without ensuring the framed reader has capacity for the entire file, a block body is likely to
/// fall across two read buffers, the decoder will not be able to decode the header, which will
/// cause it to fail.
///
/// It's recommended to use [`with_capacity`](tokio_util::codec::FramedRead::with_capacity) to set
/// the capacity of the framed reader to the size of the file.
pub(crate) struct BlockFileCodec;

impl Decoder for BlockFileCodec {
    type Item = RawBlockBody;
    type Error = FileClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }
        let mut buf_slice = &mut src.as_ref();
        let body = RawBlockBody::decode(buf_slice)?;
        src.advance(src.len() - buf_slice.len());
        Ok(Some(body))
    }
}

impl Encoder<RawBlockBody> for BlockFileCodec {
    type Error = FileClientError;

    fn encode(&mut self, item: RawBlockBody, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}
