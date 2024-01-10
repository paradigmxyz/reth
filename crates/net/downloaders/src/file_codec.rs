//! Codec for reading raw block bodies from a file.

use crate::file_client::FileClientError;
use alloy_rlp::{Decodable, Encodable};
use reth_primitives::{
    bytes::{Buf, BytesMut},
    Block,
};
use tokio_util::codec::{Decoder, Encoder};

/// Codec for reading raw block bodies from a file.
///
/// If using with [`FramedRead`](tokio_util::codec::FramedRead), the user should make sure the
/// framed reader has capacity for the entire block file. Otherwise, the decoder will return
/// [`InputTooShort`](alloy_rlp::Error::InputTooShort), because RLP headers can only be
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
    type Item = Block;
    type Error = FileClientError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None)
        }
        let buf_slice = &mut src.as_ref();
        let body = Block::decode(buf_slice)?;
        src.advance(src.len() - buf_slice.len());
        Ok(Some(body))
    }
}

impl Encoder<Block> for BlockFileCodec {
    type Error = FileClientError;

    fn encode(&mut self, item: Block, dst: &mut BytesMut) -> Result<(), Self::Error> {
        item.encode(dst);
        Ok(())
    }
}
