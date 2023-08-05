use crate::types::*;
use arrayvec::ArrayVec;
use auto_impl::auto_impl;
use bytes::{BufMut, Bytes, BytesMut};
use core::borrow::Borrow;

macro_rules! to_be_bytes_trimmed {
    ($be:ident, $x:expr) => {{
        $be = $x.to_be_bytes();
        &$be[($x.leading_zeros() / 8) as usize..]
    }};
}

impl Header {
    /// Encodes the header into the `out` buffer.
    pub fn encode(&self, out: &mut dyn BufMut) {
        if self.payload_length < 56 {
            let code = if self.list { EMPTY_LIST_CODE } else { EMPTY_STRING_CODE };
            out.put_u8(code + self.payload_length as u8);
        } else {
            let len_be;
            let len_be = to_be_bytes_trimmed!(len_be, self.payload_length);
            let code = if self.list { 0xF7 } else { 0xB7 };
            out.put_u8(code + len_be.len() as u8);
            out.put_slice(len_be);
        }
    }

    /// Returns the length of the encoded header
    pub fn length(&self) -> usize {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        out.len()
    }
}

pub const fn length_of_length(payload_length: usize) -> usize {
    if payload_length < 56 {
        1
    } else {
        1 + 8 - payload_length.leading_zeros() as usize / 8
    }
}

#[doc(hidden)]
pub const fn const_add(a: usize, b: usize) -> usize {
    a + b
}

#[doc(hidden)]
pub unsafe trait MaxEncodedLen<const LEN: usize>: Encodable {}

#[doc(hidden)]
pub unsafe trait MaxEncodedLenAssoc: Encodable {
    const LEN: usize;
}

/// Use this to define length of an encoded entity
///
/// # Safety
/// Invalid value can cause the encoder to crash.
#[macro_export]
macro_rules! impl_max_encoded_len {
    ($t:ty, $len:block) => {
        unsafe impl MaxEncodedLen<{ $len }> for $t {}
        unsafe impl MaxEncodedLenAssoc for $t {
            const LEN: usize = $len;
        }
    };
}

#[auto_impl(&)]
#[cfg_attr(feature = "alloc", auto_impl(Box, Arc))]
pub trait Encodable {
    /// Appends the rlp encoded object to the specified output buffer.
    fn encode(&self, out: &mut dyn BufMut);

    /// Returns the length of the encoded object.
    ///
    /// NOTE: This includes the length of the rlp [Header].
    fn length(&self) -> usize {
        let mut out = BytesMut::new();
        self.encode(&mut out);
        out.len()
    }
}

impl<'a> Encodable for &'a [u8] {
    fn encode(&self, out: &mut dyn BufMut) {
        if self.len() != 1 || self[0] >= EMPTY_STRING_CODE {
            Header { list: false, payload_length: self.len() }.encode(out);
        }
        out.put_slice(self);
    }

    fn length(&self) -> usize {
        let mut len = self.len();
        if self.len() != 1 || self[0] >= EMPTY_STRING_CODE {
            len += length_of_length(self.len());
        }
        len
    }
}

impl<const LEN: usize> Encodable for [u8; LEN] {
    fn encode(&self, out: &mut dyn BufMut) {
        (self as &[u8]).encode(out)
    }

    fn length(&self) -> usize {
        (self as &[u8]).length()
    }
}

unsafe impl<const LEN: usize> MaxEncodedLenAssoc for [u8; LEN] {
    const LEN: usize = LEN + length_of_length(LEN);
}

macro_rules! encodable_uint {
    ($t:ty) => {
        #[allow(clippy::cmp_owned)]
        impl Encodable for $t {
            fn length(&self) -> usize {
                if *self < <$t>::from(EMPTY_STRING_CODE) {
                    1
                } else {
                    1 + (<$t>::BITS as usize / 8) - (self.leading_zeros() as usize / 8)
                }
            }

            fn encode(&self, out: &mut dyn BufMut) {
                if *self == 0 {
                    out.put_u8(EMPTY_STRING_CODE);
                } else if *self < <$t>::from(EMPTY_STRING_CODE) {
                    out.put_u8(u8::try_from(*self).unwrap());
                } else {
                    let be;
                    let be = to_be_bytes_trimmed!(be, *self);
                    out.put_u8(EMPTY_STRING_CODE + be.len() as u8);
                    out.put_slice(be);
                }
            }
        }
    };
}

macro_rules! max_encoded_len_uint {
    ($t:ty) => {
        impl_max_encoded_len!($t, {
            length_of_length(<$t>::MAX.to_be_bytes().len()) + <$t>::MAX.to_be_bytes().len()
        });
    };
}

encodable_uint!(usize);
max_encoded_len_uint!(usize);

encodable_uint!(u8);
max_encoded_len_uint!(u8);

encodable_uint!(u16);
max_encoded_len_uint!(u16);

encodable_uint!(u32);
max_encoded_len_uint!(u32);

encodable_uint!(u64);
max_encoded_len_uint!(u64);

encodable_uint!(u128);
max_encoded_len_uint!(u128);

impl Encodable for bool {
    fn encode(&self, out: &mut dyn BufMut) {
        (*self as u8).encode(out)
    }

    fn length(&self) -> usize {
        (*self as u8).length()
    }
}

impl_max_encoded_len!(bool, { <u8 as MaxEncodedLenAssoc>::LEN });

#[cfg(feature = "smol_str")]
impl Encodable for smol_str::SmolStr {
    fn encode(&self, out: &mut dyn BufMut) {
        self.as_bytes().encode(out);
    }
    fn length(&self) -> usize {
        self.as_bytes().length()
    }
}

#[cfg(feature = "std")]
impl Encodable for std::net::IpAddr {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            std::net::IpAddr::V4(ref o) => (&o.octets()[..]).encode(out),
            std::net::IpAddr::V6(ref o) => (&o.octets()[..]).encode(out),
        }
    }
}

#[cfg(feature = "ethnum")]
mod ethnum_support {
    use super::*;

    encodable_uint!(ethnum::U256);
    impl_max_encoded_len!(ethnum::U256, { length_of_length(32) + 32 });
}

#[cfg(feature = "ethereum-types")]
mod ethereum_types_support {
    use super::*;
    use ethereum_types::*;

    use revm_primitives::{ruint::aliases::U128 as RU128, B160, B256, U256 as RU256};

    macro_rules! fixed_hash_impl {
        ($t:ty) => {
            impl Encodable for $t {
                fn length(&self) -> usize {
                    self.0.length()
                }

                fn encode(&self, out: &mut dyn bytes::BufMut) {
                    self.0.encode(out)
                }
            }
            impl_max_encoded_len!($t, { length_of_length(<$t>::len_bytes()) + <$t>::len_bytes() });
        };
    }

    fixed_hash_impl!(B160);
    fixed_hash_impl!(B256);
    fixed_hash_impl!(H64);
    fixed_hash_impl!(H128);
    fixed_hash_impl!(H160);
    fixed_hash_impl!(H256);
    fixed_hash_impl!(H512);
    fixed_hash_impl!(H520);

    macro_rules! fixed_uint_impl {
        ($t:ty, $n_bytes:tt) => {
            impl Encodable for $t {
                fn length(&self) -> usize {
                    if *self < <$t>::from(EMPTY_STRING_CODE) {
                        1
                    } else {
                        1 + $n_bytes - (self.leading_zeros() as usize / 8)
                    }
                }

                fn encode(&self, out: &mut dyn bytes::BufMut) {
                    let mut temp_arr = [0u8; $n_bytes];
                    self.to_big_endian(&mut temp_arr[..]);
                    // cut the leading zeros after converting to big endian
                    let sliced = &temp_arr[(self.leading_zeros() / 8) as usize..];
                    sliced.encode(out);
                }
            }
        };
    }

    fixed_uint_impl!(U64, 8);
    fixed_uint_impl!(U128, 16);
    fixed_uint_impl!(U256, 32);
    fixed_uint_impl!(U512, 64);

    macro_rules! fixed_revm_uint_impl {
        ($t:ty, $n_bytes:tt) => {
            impl Encodable for $t {
                fn length(&self) -> usize {
                    if *self < <$t>::from(EMPTY_STRING_CODE) {
                        1
                    } else {
                        1 + self.byte_len()
                    }
                }

                fn encode(&self, out: &mut dyn bytes::BufMut) {
                    let be = self.to_be_bytes::<$n_bytes>();
                    (&be[self.leading_zeros() / 8..]).encode(out);
                }
            }
        };
    }

    fixed_revm_uint_impl!(RU128, 16);
    fixed_revm_uint_impl!(RU256, 32);
    impl_max_encoded_len!(RU256, { length_of_length(32) + 32 });
}

macro_rules! slice_impl {
    ($t:ty) => {
        impl $crate::Encodable for $t {
            fn length(&self) -> usize {
                (&self[..]).length()
            }

            fn encode(&self, out: &mut dyn bytes::BufMut) {
                (&self[..]).encode(out)
            }
        }
    };
}

#[cfg(feature = "alloc")]
mod alloc_support {
    use super::*;

    extern crate alloc;

    impl<T> Encodable for ::alloc::vec::Vec<T>
    where
        T: Encodable,
    {
        fn encode(&self, out: &mut dyn BufMut) {
            encode_list(self, out)
        }

        fn length(&self) -> usize {
            list_length(self)
        }
    }

    impl Encodable for ::alloc::string::String {
        fn encode(&self, out: &mut dyn BufMut) {
            self.as_bytes().encode(out);
        }
        fn length(&self) -> usize {
            self.as_bytes().length()
        }
    }
}

impl Encodable for &str {
    fn encode(&self, out: &mut dyn BufMut) {
        self.as_bytes().encode(out);
    }
    fn length(&self) -> usize {
        self.as_bytes().length()
    }
}

slice_impl!(Bytes);
slice_impl!(BytesMut);

fn rlp_list_header<E, K>(v: &[K]) -> Header
where
    E: Encodable + ?Sized,
    K: Borrow<E>,
{
    let mut h = Header { list: true, payload_length: 0 };
    for x in v {
        h.payload_length += x.borrow().length();
    }
    h
}

pub fn list_length<E, K>(v: &[K]) -> usize
where
    E: Encodable,
    K: Borrow<E>,
{
    let payload_length = rlp_list_header(v).payload_length;
    length_of_length(payload_length) + payload_length
}

/// RLP encode the list of items.
pub fn encode_list<E, K>(v: &[K], out: &mut dyn BufMut)
where
    E: Encodable + ?Sized,
    K: Borrow<E>,
{
    let h = rlp_list_header(v);
    h.encode(out);
    for x in v {
        x.borrow().encode(out);
    }
}

/// RLP encode an iterator over items.
///
/// NOTE: This function clones the iterator. If the items are expensive to clone, consider
/// using [encode_list] instead.
pub fn encode_iter<K>(i: impl Iterator<Item = K> + Clone, out: &mut dyn BufMut)
where
    K: Encodable,
{
    let mut h = Header { list: true, payload_length: 0 };
    for x in i.clone() {
        h.payload_length += x.length();
    }

    h.encode(out);
    for x in i {
        x.encode(out);
    }
}

pub fn encode_fixed_size<E: MaxEncodedLen<LEN>, const LEN: usize>(v: &E) -> ArrayVec<u8, LEN> {
    let mut out = ArrayVec::from([0_u8; LEN]);

    let mut s = out.as_mut_slice();

    v.encode(&mut s);

    let final_len = LEN - s.len();
    out.truncate(final_len);

    out
}

#[cfg(feature = "kzg")]
mod kzg_support {
    extern crate c_kzg;

    use super::BufMut;
    use crate::{Decodable, DecodeError, Encodable};
    use c_kzg::{Blob, Bytes48, KzgCommitment, KzgProof, BYTES_PER_BLOB, BYTES_PER_COMMITMENT};
    use core::ops::Deref;

    impl Encodable for Blob {
        fn encode(&self, out: &mut dyn BufMut) {
            // Deref is implemented to get the underlying bytes
            self.deref().encode(out);
        }

        fn length(&self) -> usize {
            self.deref().length()
        }
    }

    impl Decodable for Blob {
        fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
            let bytes: [u8; BYTES_PER_BLOB] = Decodable::decode(buf)?;
            Ok(Blob::from(bytes))
        }
    }

    impl Encodable for Bytes48 {
        fn encode(&self, out: &mut dyn BufMut) {
            self.deref().encode(out);
        }

        fn length(&self) -> usize {
            self.deref().length()
        }
    }

    impl Decodable for Bytes48 {
        fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
            let bytes: [u8; BYTES_PER_COMMITMENT] = Decodable::decode(buf)?;
            Ok(Bytes48::from(bytes))
        }
    }

    /// Only [Encodable] is implemented for [KzgCommitment] because this is a validated type - it
    /// should be decoded using [Decodable] into a [Bytes48] type, validated, _then_ converted
    /// into a [KzgCommitment].
    impl Encodable for KzgCommitment {
        fn encode(&self, out: &mut dyn BufMut) {
            self.deref().encode(out);
        }

        fn length(&self) -> usize {
            self.deref().length()
        }
    }

    /// Only [Encodable] is implemented for [KzgProof] because this is a validated type - it should
    /// be decoded using [Decodable] into a [Bytes48] type, validated, _then_ converted into a
    /// [KzgProof].
    impl Encodable for KzgProof {
        fn encode(&self, out: &mut dyn BufMut) {
            self.deref().encode(out);
        }

        fn length(&self) -> usize {
            self.deref().length()
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::vec;
    use bytes::BytesMut;
    use hex_literal::hex;

    fn encoded<T: Encodable>(t: T) -> BytesMut {
        let mut out = BytesMut::new();
        t.encode(&mut out);
        out
    }

    fn encoded_list<T: Encodable + Clone>(t: &[T]) -> BytesMut {
        let mut out1 = BytesMut::new();
        encode_list(t, &mut out1);

        let v = t.to_vec();
        assert_eq!(out1.len(), v.length());

        let mut out2 = BytesMut::new();
        v.encode(&mut out2);
        assert_eq!(out1, out2);

        out1
    }

    fn encoded_iter<'a, T: Encodable + 'a>(iter: impl Iterator<Item = &'a T> + Clone) -> BytesMut {
        let mut out = BytesMut::new();
        encode_iter(iter, &mut out);
        out
    }

    #[test]
    fn rlp_str() {
        assert_eq!(encoded("")[..], hex!("80")[..]);
        assert_eq!(encoded("{")[..], hex!("7b")[..]);
        assert_eq!(encoded("test str")[..], hex!("887465737420737472")[..]);
    }

    #[test]
    fn rlp_strings() {
        assert_eq!(encoded(hex!(""))[..], hex!("80")[..]);
        assert_eq!(encoded(hex!("7B"))[..], hex!("7b")[..]);
        assert_eq!(encoded(hex!("80"))[..], hex!("8180")[..]);
        assert_eq!(encoded(hex!("ABBA"))[..], hex!("82abba")[..]);
    }

    fn u8_fixtures() -> impl IntoIterator<Item = (u8, &'static [u8])> {
        vec![
            (0, &hex!("80")[..]),
            (1, &hex!("01")[..]),
            (0x7F, &hex!("7F")[..]),
            (0x80, &hex!("8180")[..]),
        ]
    }

    fn c<T, U: From<T>>(
        it: impl IntoIterator<Item = (T, &'static [u8])>,
    ) -> impl Iterator<Item = (U, &'static [u8])> {
        it.into_iter().map(|(k, v)| (k.into(), v))
    }

    fn u16_fixtures() -> impl IntoIterator<Item = (u16, &'static [u8])> {
        c(u8_fixtures()).chain(vec![(0x400, &hex!("820400")[..])])
    }

    fn u32_fixtures() -> impl IntoIterator<Item = (u32, &'static [u8])> {
        c(u16_fixtures())
            .chain(vec![(0xFFCCB5, &hex!("83ffccb5")[..]), (0xFFCCB5DD, &hex!("84ffccb5dd")[..])])
    }

    fn u64_fixtures() -> impl IntoIterator<Item = (u64, &'static [u8])> {
        c(u32_fixtures()).chain(vec![
            (0xFFCCB5DDFF, &hex!("85ffccb5ddff")[..]),
            (0xFFCCB5DDFFEE, &hex!("86ffccb5ddffee")[..]),
            (0xFFCCB5DDFFEE14, &hex!("87ffccb5ddffee14")[..]),
            (0xFFCCB5DDFFEE1483, &hex!("88ffccb5ddffee1483")[..]),
        ])
    }

    fn u128_fixtures() -> impl IntoIterator<Item = (u128, &'static [u8])> {
        c(u64_fixtures()).chain(vec![(
            0x10203E405060708090A0B0C0D0E0F2,
            &hex!("8f10203e405060708090a0b0c0d0e0f2")[..],
        )])
    }

    #[cfg(feature = "ethnum")]
    fn u256_fixtures() -> impl IntoIterator<Item = (ethnum::U256, &'static [u8])> {
        c(u128_fixtures()).chain(vec![(
            ethnum::U256::from_str_radix(
                "0100020003000400050006000700080009000A0B4B000C000D000E01",
                16,
            )
            .unwrap(),
            &hex!("9c0100020003000400050006000700080009000a0b4b000c000d000e01")[..],
        )])
    }

    #[cfg(feature = "ethereum-types")]
    fn eth_u64_fixtures() -> impl IntoIterator<Item = (ethereum_types::U64, &'static [u8])> {
        c(u64_fixtures()).chain(vec![
            (
                ethereum_types::U64::from_str_radix("FFCCB5DDFF", 16).unwrap(),
                &hex!("85ffccb5ddff")[..],
            ),
            (
                ethereum_types::U64::from_str_radix("FFCCB5DDFFEE", 16).unwrap(),
                &hex!("86ffccb5ddffee")[..],
            ),
            (
                ethereum_types::U64::from_str_radix("FFCCB5DDFFEE14", 16).unwrap(),
                &hex!("87ffccb5ddffee14")[..],
            ),
            (
                ethereum_types::U64::from_str_radix("FFCCB5DDFFEE1483", 16).unwrap(),
                &hex!("88ffccb5ddffee1483")[..],
            ),
        ])
    }

    #[cfg(feature = "ethereum-types")]
    fn eth_u128_fixtures() -> impl IntoIterator<Item = (ethereum_types::U128, &'static [u8])> {
        c(u128_fixtures()).chain(vec![(
            ethereum_types::U128::from_str_radix("10203E405060708090A0B0C0D0E0F2", 16).unwrap(),
            &hex!("8f10203e405060708090a0b0c0d0e0f2")[..],
        )])
    }

    #[cfg(feature = "ethereum-types")]
    fn eth_u256_fixtures() -> impl IntoIterator<Item = (ethereum_types::U256, &'static [u8])> {
        c(u128_fixtures()).chain(vec![(
            ethereum_types::U256::from_str_radix(
                "0100020003000400050006000700080009000A0B4B000C000D000E01",
                16,
            )
            .unwrap(),
            &hex!("9c0100020003000400050006000700080009000a0b4b000c000d000e01")[..],
        )])
    }

    #[cfg(feature = "ethereum-types")]
    fn eth_u512_fixtures() -> impl IntoIterator<Item = (ethereum_types::U512, &'static [u8])> {
        c(eth_u256_fixtures()).chain(vec![(
            ethereum_types::U512::from_str_radix(
                "0100020003000400050006000700080009000A0B4B000C000D000E010100020003000400050006000700080009000A0B4B000C000D000E01",
                16,
            )
                .unwrap(),
            &hex!("b8380100020003000400050006000700080009000A0B4B000C000D000E010100020003000400050006000700080009000A0B4B000C000D000E01")[..],
        )])
    }

    macro_rules! uint_rlp_test {
        ($fixtures:expr) => {
            for (input, output) in $fixtures {
                assert_eq!(encoded(input), output);
            }
        };
    }

    #[test]
    fn rlp_uints() {
        uint_rlp_test!(u8_fixtures());
        uint_rlp_test!(u16_fixtures());
        uint_rlp_test!(u32_fixtures());
        uint_rlp_test!(u64_fixtures());
        uint_rlp_test!(u128_fixtures());
        #[cfg(feature = "ethnum")]
        uint_rlp_test!(u256_fixtures());
    }

    #[cfg(feature = "ethereum-types")]
    #[test]
    fn rlp_eth_uints() {
        uint_rlp_test!(eth_u64_fixtures());
        uint_rlp_test!(eth_u128_fixtures());
        uint_rlp_test!(eth_u256_fixtures());
        uint_rlp_test!(eth_u512_fixtures());
    }

    #[test]
    fn rlp_list() {
        assert_eq!(encoded_list::<u64>(&[]), &hex!("c0")[..]);
        assert_eq!(encoded_list::<u8>(&[0x00u8]), &hex!("c180")[..]);
        assert_eq!(encoded_list(&[0xFFCCB5_u64, 0xFFC0B5_u64]), &hex!("c883ffccb583ffc0b5")[..]);
    }

    #[test]
    fn rlp_iter() {
        assert_eq!(encoded_iter::<u64>([].iter()), &hex!("c0")[..]);
        assert_eq!(
            encoded_iter([0xFFCCB5_u64, 0xFFC0B5_u64].iter()),
            &hex!("c883ffccb583ffc0b5")[..]
        );
    }

    #[cfg(feature = "smol_str")]
    #[test]
    fn rlp_smol_str() {
        use smol_str::SmolStr;
        assert_eq!(encoded(SmolStr::new(""))[..], hex!("80")[..]);
        let mut b = BytesMut::new();
        "test smol str".to_string().encode(&mut b);
        assert_eq!(&encoded(SmolStr::new("test smol str"))[..], b.as_ref());
        let mut b = BytesMut::new();
        "abcdefgh".to_string().encode(&mut b);
        assert_eq!(&encoded(SmolStr::new("abcdefgh"))[..], b.as_ref());
    }

    #[test]
    fn to_be_bytes_trimmed() {
        macro_rules! test_to_be_bytes_trimmed {
            ($($x:expr => $expected:expr),+ $(,)?) => {$(
                let be;
                assert_eq!(to_be_bytes_trimmed!(be, $x), $expected);
            )+};
        }

        test_to_be_bytes_trimmed! {
            0u8 => [],
            0u16 => [],
            0u32 => [],
            0u64 => [],
            0usize => [],
            0u128 => [],

            1u8 => [1],
            1u16 => [1],
            1u32 => [1],
            1u64 => [1],
            1usize => [1],
            1u128 => [1],

            u8::MAX => [0xff],
            u16::MAX => [0xff, 0xff],
            u32::MAX => [0xff, 0xff, 0xff, 0xff],
            u64::MAX => [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            u128::MAX => [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],

            1u8 => [1],
            255u8 => [255],
            256u16 => [1, 0],
            65535u16 => [255, 255],
            65536u32 => [1, 0, 0],
            65536u64 => [1, 0, 0],
        }
    }
}
