//! Codec for reading raw block bodies from a file.
use super::FileClientError;
use bytes::{Buf, BytesMut};
use reth_eth_wire::RawBlockBody;
use reth_rlp::{Decodable, Encodable};
use tokio_util::codec::{Decoder, Encoder};

/// Codec for reading raw block bodies from a file.
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
