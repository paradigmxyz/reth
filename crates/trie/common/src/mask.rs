use super::TrieMask;
use bytes::Buf;
use reth_codecs::Compact;

pub(crate) struct StoredTrieMask(pub(crate) TrieMask);

impl Compact for StoredTrieMask {
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_u16(self.0.get());
        2
    }

    fn from_compact(mut buf: &[u8], _len: usize) -> (Self, &[u8]) {
        let mask = buf.get_u16();
        (Self(TrieMask::new(mask)), buf)
    }
}
