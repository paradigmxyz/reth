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
    /// Returns 0 since we won't include it in the `StructFlags`.
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
    /// Returns 0 for `None` and 1 for `Some(_)`.
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
    /// `bool` vars go directly to the `StructFlags` and are not written to the buffer.
    fn to_compact(self, _: &mut impl bytes::BufMut) -> usize {
        self as usize
    }

    /// `bool` expects the real value to come in `len`, and does not advance the cursor.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        (len != 0, buf)
    }
}

mod tests {
    use super::*;
    use ethers_core::types::Address;

    #[test]
    fn compact_bytes() {
        let arr = [1, 2, 3, 4, 5];
        let mut list = bytes::Bytes::copy_from_slice(&arr);
        let mut buf = vec![];
        assert_eq!(list.clone().to_compact(&mut buf), list.len());

        // Add some noise data.
        buf.push(1);

        assert_eq!(&buf[..arr.len()], &arr);
        assert_eq!(bytes::Bytes::from_compact(&buf, list.len()), (list, vec![1].as_slice()));
    }

    #[test]
    fn compact_bloom() {
        let mut buf = vec![];
        assert_eq!(Bloom::default().to_compact(&mut buf), 256);
        assert_eq!(buf, vec![0; 256]);

        // Add some noise data.
        buf.push(1);

        // Bloom shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(Bloom::from_compact(&buf, 1000), (Bloom::default(), vec![1u8].as_slice()));
    }

    #[test]
    fn compact_address() {
        let mut buf = vec![];
        assert_eq!(Address::zero().to_compact(&mut buf), 20);
        assert_eq!(buf, vec![0; 20]);

        // Add some noise data.
        buf.push(1);

        // Address shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(Address::from_compact(&buf, 1000), (Address::zero(), vec![1u8].as_slice()));
    }

    #[test]
    fn compact_H256() {
        let mut buf = vec![];
        assert_eq!(H256::zero().to_compact(&mut buf), 32);
        assert_eq!(buf, vec![0; 32]);

        // Add some noise data.
        buf.push(1);

        // H256 shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(H256::from_compact(&buf, 1000), (H256::zero(), vec![1u8].as_slice()));
    }

    #[test]
    fn compact_bool() {
        let vtrue = true;
        let mut buf = vec![];

        assert_eq!(true.to_compact(&mut buf), 1);
        // Bool vars go directly to the `StructFlags` and not written to the buf.
        assert_eq!(buf.len(), 0);

        assert_eq!(false.to_compact(&mut buf), 0);
        assert_eq!(buf.len(), 0);

        let mut buf = vec![100u8];

        // Bool expects the real value to come in `len`, and does not advance the cursor.
        assert_eq!(bool::from_compact(&buf, 1), (true, buf.as_slice()));
        assert_eq!(bool::from_compact(&buf, 0), (false, buf.as_slice()));
    }

    #[test]
    fn compact_option() {
        let opt = Some(H256::zero());
        let mut buf = vec![];

        assert_eq!(None::<H256>.to_compact(&mut buf), 0);
        assert_eq!(opt.clone().to_compact(&mut buf), 1);

        assert_eq!(Option::<H256>::from_compact(&buf, 1), (opt, vec![].as_slice()));

        // If `None`, it returns the slice at the same cursor position.
        assert_eq!(Option::<H256>::from_compact(&buf, 0), (None, buf.as_slice()));
    }

    #[test]
    fn compact_vec() {
        let list = vec![H256::zero(), H256::zero()];
        let mut buf = vec![];

        // Vec doesn't return a total length
        assert_eq!(list.clone().to_compact(&mut buf), 0);

        // Add some noise data in the end that should be returned by `from_compact`.
        buf.extend(&[1u8, 2]);

        let mut remaining_buf = buf.as_slice();
        remaining_buf.advance(2 + 4 + 32 + 4 + 32);

        assert_eq!(Vec::<H256>::from_compact(&buf, 0), (list, remaining_buf));
        assert_eq!(remaining_buf, &[1u8, 2]);
    }

    #[test]
    fn compact_U256() {
        let mut buf = vec![];

        assert_eq!(U256::zero().to_compact(&mut buf), 0);
        assert!(buf.is_empty());
        assert_eq!(U256::from_compact(&buf, 0), (U256::zero(), vec![].as_slice()));

        assert_eq!(U256::from(2).to_compact(&mut buf), 1);
        assert_eq!(buf, vec![2u8]);
        assert_eq!(U256::from_compact(&buf, 1), (U256::from(2), vec![].as_slice()));
    }

    #[test]
    fn compact_u64() {
        let mut buf = vec![];

        assert_eq!(0u64.to_compact(&mut buf), 0);
        assert!(buf.is_empty());
        assert_eq!(u64::from_compact(&buf, 0), (0u64, vec![].as_slice()));

        assert_eq!(2u64.to_compact(&mut buf), 1);
        assert_eq!(buf, vec![2u8]);
        assert_eq!(u64::from_compact(&buf, 1), (2u64, vec![].as_slice()));

        let mut buf = vec![];

        assert_eq!(0xffffffffffffffffu64.to_compact(&mut buf), 8);
        assert_eq!(&buf, &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(u64::from_compact(&buf, 8), (0xffffffffffffffffu64, vec![].as_slice()));
    }
}
