//! Compact codec.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![warn(unused_crate_dependencies)]
// TODO: remove when https://github.com/proptest-rs/proptest/pull/427 is merged
#![allow(unknown_lints, non_local_definitions)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

pub use codecs_derive::*;

use alloy_primitives::{Address, Bloom, Bytes, B256, B512, U256};
use bytes::Buf;

/// Trait that implements the `Compact` codec.
///
/// When deriving the trait for custom structs, be aware of certain limitations/recommendations:
/// * Works best with structs that only have native types (eg. u64, B256, U256).
/// * Fixed array types (B256, Address, Bloom) are not compacted.
/// * Max size of `T` in `Option<T>` or `Vec<T>` shouldn't exceed `0xffff`.
/// * Any `Bytes` field **should be placed last**.
/// * Any other type which is not known to the derive module **should be placed last** in they
///   contain a `Bytes` field.
///
/// The last two points make it easier to decode the data without saving the length on the
/// `StructFlags`. It will fail compilation if it's not respected. If they're alias to known types,
/// add their definitions to `get_bit_size()` or `known_types` in `generator.rs`.
///
/// Regarding the `specialized_to/from_compact` methods: Mainly used as a workaround for not being
/// able to specialize an impl over certain types like `Vec<T>`/`Option<T>` where `T` is a fixed
/// size array like `Vec<B256>`.
pub trait Compact: Sized {
    /// Takes a buffer which can be written to. *Ideally*, it returns the length written to.
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>;

    /// Takes a buffer which can be read from. Returns the object and `buf` with its internal cursor
    /// advanced (eg.`.advance(len)`).
    ///
    /// `len` can either be the `buf` remaining length, or the length of the compacted type.
    ///
    /// It will panic, if `len` is smaller than `buf.len()`.
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]);

    /// "Optional": If there's no good reason to use it, don't.
    #[inline]
    fn specialized_to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self.to_compact(buf)
    }

    /// "Optional": If there's no good reason to use it, don't.
    #[inline]
    fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        Self::from_compact(buf, len)
    }
}

macro_rules! impl_uint_compact {
    ($($name:tt),+) => {
        $(
            impl Compact for $name {
                #[inline]
                fn to_compact<B>(self, buf: &mut B) -> usize
                    where B: bytes::BufMut + AsMut<[u8]>
                {
                    let leading = self.leading_zeros() as usize / 8;
                    buf.put_slice(&self.to_be_bytes()[leading..]);
                    core::mem::size_of::<$name>() - leading
                }

                #[inline]
                fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
                    if len == 0 {
                        return (0, buf);
                    }

                    let mut arr = [0; core::mem::size_of::<$name>()];
                    arr[core::mem::size_of::<$name>() - len..].copy_from_slice(&buf[..len]);
                    buf.advance(len);
                    ($name::from_be_bytes(arr), buf)
                }
            }
        )+
    };
}

impl_uint_compact!(u8, u64, u128);

impl<T> Compact for Vec<T>
where
    T: Compact,
{
    /// Returns 0 since we won't include it in the `StructFlags`.
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        encode_varuint(self.len(), buf);

        let mut tmp: Vec<u8> = Vec::with_capacity(64);

        for element in self {
            tmp.clear();

            // We don't know the length until we compact it
            let length = element.to_compact(&mut tmp);
            encode_varuint(length, buf);

            buf.put_slice(&tmp);
        }

        0
    }

    #[inline]
    fn from_compact(buf: &[u8], _: usize) -> (Self, &[u8]) {
        let (length, mut buf) = decode_varuint(buf);
        let mut list = Vec::with_capacity(length);
        for _ in 0..length {
            let len;
            (len, buf) = decode_varuint(buf);

            let (element, _) = T::from_compact(&buf[..len], len);
            buf.advance(len);

            list.push(element);
        }

        (list, buf)
    }

    /// To be used by fixed sized types like `Vec<B256>`.
    #[inline]
    fn specialized_to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        encode_varuint(self.len(), buf);
        for element in self {
            element.to_compact(buf);
        }
        0
    }

    /// To be used by fixed sized types like `Vec<B256>`.
    #[inline]
    fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (length, mut buf) = decode_varuint(buf);
        let mut list = Vec::with_capacity(length);

        for _ in 0..length {
            let element;
            (element, buf) = T::from_compact(buf, len);
            list.push(element);
        }

        (list, buf)
    }
}

impl<T> Compact for Option<T>
where
    T: Compact,
{
    /// Returns 0 for `None` and 1 for `Some(_)`.
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let Some(element) = self else { return 0 };

        // We don't know the length of the element until we compact it.
        let mut tmp = Vec::with_capacity(64);
        let length = element.to_compact(&mut tmp);

        encode_varuint(length, buf);

        buf.put_slice(&tmp);

        1
    }

    #[inline]
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }

        let (len, mut buf) = decode_varuint(buf);

        let (element, _) = T::from_compact(&buf[..len], len);
        buf.advance(len);

        (Some(element), buf)
    }

    /// To be used by fixed sized types like `Option<B256>`.
    #[inline]
    fn specialized_to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        if let Some(element) = self {
            element.to_compact(buf);
            1
        } else {
            0
        }
    }

    /// To be used by fixed sized types like `Option<B256>`.
    #[inline]
    fn specialized_from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (None, buf)
        }

        let (element, buf) = T::from_compact(buf, len);
        (Some(element), buf)
    }
}

impl Compact for U256 {
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let inner = self.to_be_bytes::<32>();
        let size = 32 - (self.leading_zeros() / 8);
        buf.put_slice(&inner[32 - size..]);
        size
    }

    #[inline]
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return (U256::ZERO, buf)
        }

        let mut arr = [0; 32];
        arr[(32 - len)..].copy_from_slice(&buf[..len]);
        buf.advance(len);
        (U256::from_be_bytes(arr), buf)
    }
}

impl Compact for Bytes {
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let len = self.len();
        buf.put(self.0);
        len
    }

    #[inline]
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        (buf.copy_to_bytes(len).into(), buf)
    }
}

impl<const N: usize> Compact for [u8; N] {
    #[inline]
    fn to_compact<B>(self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        buf.put_slice(&self);
        N
    }

    #[inline]
    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        if len == 0 {
            return ([0; N], buf)
        }

        let v = buf[..N].try_into().unwrap();
        buf.advance(N);
        (v, buf)
    }
}

/// Implements the [`Compact`] trait for fixed size byte array types like [`B256`].
#[macro_export]
macro_rules! impl_compact_for_bytes {
    ($($name:tt),+) => {
        $(
            impl Compact for $name {
                #[inline]
                fn to_compact<B>(self, buf: &mut B) -> usize
                where
                    B: bytes::BufMut + AsMut<[u8]>
                {
                    self.0.to_compact(buf)
                }

                #[inline]
                fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
                    let (v, buf) = <[u8; core::mem::size_of::<$name>()]>::from_compact(buf, len);
                    (Self::from(v), buf)
                }
            }
        )+
    };
}

impl_compact_for_bytes!(Address, B256, B512, Bloom);

impl Compact for bool {
    /// `bool` vars go directly to the `StructFlags` and are not written to the buffer.
    #[inline]
    fn to_compact<B>(self, _: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        self as usize
    }

    /// `bool` expects the real value to come in `len`, and does not advance the cursor.
    #[inline]
    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        (len != 0, buf)
    }
}

fn encode_varuint<B>(mut n: usize, buf: &mut B)
where
    B: bytes::BufMut + AsMut<[u8]>,
{
    while n >= 0x80 {
        buf.put_u8((n as u8) | 0x80);
        n >>= 7;
    }
    buf.put_u8(n as u8);
}

fn decode_varuint(buf: &[u8]) -> (usize, &[u8]) {
    let mut value = 0;

    for i in 0..33 {
        let byte = buf[i];
        value |= usize::from(byte & 0x7F) << (i * 7);
        if byte < 0x80 {
            return (value, &buf[i + 1..])
        }
    }

    decode_varuint_panic();
}

#[inline(never)]
#[cold]
const fn decode_varuint_panic() -> ! {
    panic!("could not decode varuint");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_bytes() {
        let arr = [1, 2, 3, 4, 5];
        let list = Bytes::copy_from_slice(&arr);
        let mut buf = vec![];
        assert_eq!(list.clone().to_compact(&mut buf), list.len());

        // Add some noise data.
        buf.push(1);

        assert_eq!(&buf[..arr.len()], &arr);
        assert_eq!(Bytes::from_compact(&buf, list.len()), (list, vec![1].as_slice()));
    }

    #[test]
    fn compact_address() {
        let mut buf = vec![];
        assert_eq!(Address::ZERO.to_compact(&mut buf), 20);
        assert_eq!(buf, vec![0; 20]);

        // Add some noise data.
        buf.push(1);

        // Address shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(Address::from_compact(&buf, 1000), (Address::ZERO, vec![1u8].as_slice()));
    }

    #[test]
    fn compact_b256() {
        let mut buf = vec![];
        assert_eq!(B256::ZERO.to_compact(&mut buf), 32);
        assert_eq!(buf, vec![0; 32]);

        // Add some noise data.
        buf.push(1);

        // B256 shouldn't care about the len passed, since it's not actually compacted.
        assert_eq!(B256::from_compact(&buf, 1000), (B256::ZERO, vec![1u8].as_slice()));
    }

    #[test]
    fn compact_bool() {
        let _vtrue = true;
        let mut buf = vec![];

        assert_eq!(true.to_compact(&mut buf), 1);
        // Bool vars go directly to the `StructFlags` and not written to the buf.
        assert_eq!(buf.len(), 0);

        assert_eq!(false.to_compact(&mut buf), 0);
        assert_eq!(buf.len(), 0);

        let buf = vec![100u8];

        // Bool expects the real value to come in `len`, and does not advance the cursor.
        assert_eq!(bool::from_compact(&buf, 1), (true, buf.as_slice()));
        assert_eq!(bool::from_compact(&buf, 0), (false, buf.as_slice()));
    }

    #[test]
    fn compact_option() {
        let opt = Some(B256::ZERO);
        let mut buf = vec![];

        assert_eq!(None::<B256>.to_compact(&mut buf), 0);
        assert_eq!(opt.to_compact(&mut buf), 1);
        assert_eq!(buf.len(), 1 + 32);

        assert_eq!(Option::<B256>::from_compact(&buf, 1), (opt, vec![].as_slice()));

        // If `None`, it returns the slice at the same cursor position.
        assert_eq!(Option::<B256>::from_compact(&buf, 0), (None, buf.as_slice()));

        let mut buf = vec![];
        assert_eq!(opt.specialized_to_compact(&mut buf), 1);
        assert_eq!(buf.len(), 32);
        assert_eq!(Option::<B256>::specialized_from_compact(&buf, 1), (opt, vec![].as_slice()));
    }

    #[test]
    fn compact_vec() {
        let list = vec![B256::ZERO, B256::ZERO];
        let mut buf = vec![];

        // Vec doesn't return a total length
        assert_eq!(list.clone().to_compact(&mut buf), 0);

        // Add some noise data in the end that should be returned by `from_compact`.
        buf.extend([1u8, 2]);

        let mut remaining_buf = buf.as_slice();
        remaining_buf.advance(1 + 1 + 32 + 1 + 32);

        assert_eq!(Vec::<B256>::from_compact(&buf, 0), (list, remaining_buf));
        assert_eq!(remaining_buf, &[1u8, 2]);
    }

    #[test]
    fn compact_u256() {
        let mut buf = vec![];

        assert_eq!(U256::ZERO.to_compact(&mut buf), 0);
        assert!(buf.is_empty());
        assert_eq!(U256::from_compact(&buf, 0), (U256::ZERO, vec![].as_slice()));

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

    #[test]
    fn variable_uint() {
        proptest::proptest!(|(val: usize)| {
            let mut buf = vec![];
            encode_varuint(val, &mut buf);
            let (decoded, read_buf) = decode_varuint(&buf);
            assert_eq!(val, decoded);
            assert!(!read_buf.has_remaining());
        });
    }

    #[main_codec]
    #[derive(Debug, PartialEq, Clone)]
    struct TestStruct {
        f_u64: u64,
        f_u256: U256,
        f_bool_t: bool,
        f_bool_f: bool,
        f_option_none: Option<B256>,
        f_option_some: Option<B256>,
        f_option_some_u64: Option<u64>,
        f_vec_empty: Vec<Address>,
        f_vec_some: Vec<Address>,
    }

    impl Default for TestStruct {
        fn default() -> Self {
            TestStruct {
                f_u64: 1u64,                                    // 4 bits | 1 byte
                f_u256: U256::from(1u64),                       // 6 bits | 1 byte
                f_bool_f: false,                                // 1 bit  | 0 bytes
                f_bool_t: true,                                 // 1 bit  | 0 bytes
                f_option_none: None,                            // 1 bit  | 0 bytes
                f_option_some: Some(B256::ZERO),                // 1 bit  | 32 bytes
                f_option_some_u64: Some(0xffffu64),             // 1 bit  | 1 + 2 bytes
                f_vec_empty: vec![],                            // 0 bits | 1 bytes
                f_vec_some: vec![Address::ZERO, Address::ZERO], // 0 bits | 1 + 20*2 bytes
            }
        }
    }

    #[test]
    fn compact_test_struct() {
        let test = TestStruct::default();
        let mut buf = vec![];
        assert_eq!(
            test.to_compact(&mut buf),
            2 + // TestStructFlags
            1 +
            1 +
            // 0 + 0 + 0 +
            32 +
            1 + 2 +
            1 +
            1 + 20 * 2
        );

        assert_eq!(
            TestStruct::from_compact(&buf, buf.len()),
            (TestStruct::default(), vec![].as_slice())
        );
    }

    #[main_codec]
    #[derive(Debug, PartialEq, Clone, Default)]
    enum TestEnum {
        #[default]
        Var0,
        Var1(TestStruct),
        Var2(u64),
    }

    #[cfg(test)]
    #[allow(dead_code)]
    #[test_fuzz::test_fuzz]
    fn compact_test_enum_all_variants(var0: TestEnum, var1: TestEnum, var2: TestEnum) {
        let mut buf = vec![];
        var0.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var0);

        let mut buf = vec![];
        var1.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var1);

        let mut buf = vec![];
        var2.clone().to_compact(&mut buf);
        assert_eq!(TestEnum::from_compact(&buf, buf.len()).0, var2);
    }

    #[test]
    fn compact_test_enum() {
        let var0 = TestEnum::Var0;
        let var1 = TestEnum::Var1(TestStruct::default());
        let var2 = TestEnum::Var2(1u64);

        compact_test_enum_all_variants(var0, var1, var2);
    }
}
