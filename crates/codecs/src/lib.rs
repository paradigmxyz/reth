use bytes::{Buf, Bytes};
pub use codecs_derive::*;

/// Trait that implements the `Compact` codec.
///
/// When deriving the trait for custom structs, be aware of certain limitations/recommendations:
/// * Works best with structs that only have native types (eg. u64, H256, U256).
/// * Fixed array types (H256, Address, Bloom) are not compacted.
/// * Any `bytes::Bytes` field **should be placed last**.
/// * Any other type which is not known to the derive module **should be placed last**.
///
/// The last two points make it easier to decode the data without saving the length on the
/// `StructFlags`.
pub trait Compact {
    /// Takes a buffer which can be written to. *Ideally*, it returns the length written to.
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize;
    /// Takes a buffer which can be read from. Returns the object and `buf` with its internal cursor
    /// advanced (eg.`.advance(len)`).
    ///
    /// `len` can either be the `buf` remaining length, or the length of the compacted type.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8])
    where
        Self: Sized;
}

impl Compact for u64 {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        let leading = self.leading_zeros() as usize / 8;
        buf.put_slice(&self.to_be_bytes()[leading..]);
        8 - leading
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len > 0 {
            let mut arr = [0; 8];
            arr[8 - len..].copy_from_slice(&buf[..len]);

            buf.advance(len);

            return (u64::from_be_bytes(arr), buf)
        }
        (0, buf)
    }
}

use ethers_core::types::{Bloom, H160, H256, U256};

impl<T> Compact for Vec<T>
where
    T: Compact + Default,
{
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        // TODO: can it be smaller?
        buf.put_u16(self.len() as u16);

        for element in self {
            // TODO: elias fano?
            let mut inner = Vec::with_capacity(32);
            buf.put_u32(element.to_compact(&mut inner) as u32);
            buf.put_slice(&inner);
        }
        0
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let mut list = vec![];
        let length = buf.get_u16();
        for _ in 0..length {
            #[allow(unused_assignments)]
            let mut element = T::default();

            let len = buf.get_u32();
            (element, buf) = T::from_compact(buf, len as usize);

            list.push(element);
        }

        (list, buf)
    }
}

impl<T> Compact for Option<T>
where
    T: Compact + Default,
{
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        if let Some(element) = self {
            let mut inner = vec![];
            let len = element.to_compact(&mut inner);
            buf.put_u16(len as u16);
            buf.put_slice(&inner);
            return 1
        }
        0
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }

        let len = buf.get_u16();
        let (element, buf) = T::from_compact(buf, len as usize);

        (Some(element), buf)
    }
}

impl Compact for U256 {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        let mut inner = vec![0; 32];
        self.to_big_endian(&mut inner);
        let size = 32 - (self.leading_zeros() / 8) as usize;
        buf.put_slice(&inner[32 - size..]);
        size
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len > 0 {
            let mut arr = [0; 32];
            arr[(32 - len)..].copy_from_slice(&buf[..len]);
            buf.advance(len);
            return (U256::from_big_endian(&arr), buf)
        }

        (U256::zero(), buf)
    }
}

impl Compact for Bytes {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        let len = self.len();
        buf.put(self);
        len
    }
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        (buf.copy_to_bytes(len), buf)
    }
}

macro_rules! impl_hash_compact {
    ($(($name:tt, $size:tt)),+) => {
        $(
            impl Compact for $name {
                fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
                    buf.put_slice(&self.0);
                    $size
                }

                fn from_compact(mut buf: &[u8], len: usize) -> (Self,&[u8]) {
                    if len == 0 {
                        return ($name::default(), buf)
                    }

                    let v = $name::from_slice(
                        buf.get(..$size).expect("size not matching"),
                    );
                    buf.advance($size);
                    (v, buf)
                }
            }
        )+
    };
}

impl_hash_compact!((H256, 32), (H160, 20));

impl Compact for Bloom {
    fn to_compact(self, buf: &mut impl bytes::BufMut) -> usize {
        buf.put_slice(&self.0);
        256
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let result = Bloom::from_slice(&buf[..256]);
        buf.advance(256);
        (result, buf)
    }
}

impl Compact for bool {
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        self as usize
    }
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        (len != 0, buf)
    }
}
