use bytes::{Buf, Bytes};
pub use codecs_derive::*;
pub trait Compact {
    type Encoded: AsRef<[u8]> + Default;

    fn to_compact(self) -> (usize, Self::Encoded);
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8])
    where
        Self: Sized;
}

impl Compact for u64 {
    type Encoded = bytes::Bytes;

    fn to_compact(self) -> (usize, Self::Encoded) {
        let buf = self.to_be_bytes().into_iter().skip_while(|&v| v == 0);
        let buf = bytes::Bytes::from_iter(buf);
        (buf.len(), buf)
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
    type Encoded = Vec<u8>;

    fn to_compact(self) -> (usize, Self::Encoded) {
        let mut buf = vec![self.len() as u8];

        for element in self {
            let (len, obj) = element.to_compact();
            buf.extend_from_slice(&obj.as_ref()[0..len]);
        }

        (buf.len(), buf)
    }

    fn from_compact(mut buf: &[u8], _: usize) -> (Self, &[u8]) {
        let mut list = vec![];
        let length = buf.get_u8();

        for _ in 0..length {
            #[allow(unused_assignments)]
            let mut element = T::default();
            (element, buf) = T::from_compact(buf, buf.len());
            list.push(element);
        }

        (list, buf)
    }
}

impl<T> Compact for Option<T>
where
    T: Compact + Default,
{
    type Encoded = <T as Compact>::Encoded;

    fn to_compact(self) -> (usize, Self::Encoded) {
        if let Some(element) = self {
            return element.to_compact()
        }
        (0, <T as Compact>::Encoded::default())
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }
        let (element, buf) = T::from_compact(buf, len);
        (Some(element), buf)
    }
}

impl Compact for U256 {
    type Encoded = Vec<u8>;

    fn to_compact(self) -> (usize, Self::Encoded) {
        let mut buf = [0u8; 32];
        self.to_big_endian(&mut buf);
        let size = 32 - (self.leading_zeros() / 8) as usize;
        (size, buf[32 - size..].to_vec())
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len > 0 {
            let mut arr = [0; 32];
            arr[(32 - len)..].copy_from_slice(&buf[..3]);
            buf.advance(len);
            return (U256::from_big_endian(&arr), buf)
        }

        (U256::zero(), buf)
    }
}

impl Compact for Bytes {
    type Encoded = bytes::Bytes;

    fn to_compact(self) -> (usize, Self::Encoded) {
        (self.len(), self)
    }
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        (buf.copy_to_bytes(len), buf)
    }
}

macro_rules! impl_hash_compact {
    ($(($name:tt, $size:tt)),+) => {
        $(
            impl Compact for $name {
                type Encoded = bytes::Bytes;

                fn to_compact(self) -> (usize, Self::Encoded) {
                    let buf = self.as_bytes().into_iter().filter_map(|v| {
                        Some(*v)
                    });
                    let buf = bytes::Bytes::from_iter(buf);
                    (buf.len(), buf)
                }
                fn from_compact(mut buf: &[u8], _: usize) -> (Self,&[u8]) {
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
    type Encoded = bytes::Bytes;

    fn to_compact(self) -> (usize, Self::Encoded) {
        let buf = self.0.into_iter().skip_while(|&v| v == 0);
        let buf = bytes::Bytes::from_iter(buf);
        (buf.len(), buf)
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (Bloom::default(), buf)
        }
        let result = Bloom::from_slice(&buf[..len]);
        buf.advance(len);
        (result, buf)
    }
}
