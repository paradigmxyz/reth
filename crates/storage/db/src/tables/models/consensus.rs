use reth_codecs::{derive_arbitrary, main_codec, Compact};
use reth_interfaces::db::DatabaseError;
use serde::Serialize;

use crate::table::{Compress, Decode, Decompress, Encode};

///
/// It is stored as the content of the Consensus.
///
#[derive_arbitrary]
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize)]
// #[main_codec]
// #[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ConsensusBytes {
    /// The content of this Consensus.
    pub content: Vec<u8>,
}

// NOTE: Removing main_codec and manually encode subkey
// and compress second part of the value. If we have compression
// over whole value (Even SubKey) that would mess up fetching of values with seek_by_key_subkey
impl Compact for ConsensusBytes {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        // for now put full bytes and later compress it.
        self.content.to_compact(buf)
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (content, buf) = Vec::<u8>::from_compact(buf, len);
        (Self { content }, buf)
    }
}

impl Encode for ConsensusBytes {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        self.content.encode()
    }
}

impl Decode for ConsensusBytes {
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DatabaseError> {
        let content = Vec::<u8>::decode(value)?;

        Ok(ConsensusBytes { content })
    }
}

impl Compress for ConsensusBytes {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.content.compress()
    }
    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        self.content.compress_to_buf(buf)
    }
}

impl Decompress for ConsensusBytes {
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, DatabaseError> {
        let content = Vec::<u8>::decompress(value)?;

        Ok(ConsensusBytes { content })
    }
}
