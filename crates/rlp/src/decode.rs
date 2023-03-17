use crate::types::Header;
use bytes::{Buf, Bytes, BytesMut};

pub trait Decodable: Sized {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError>;
}

#[cfg(feature = "alloc")]
mod alloc_impl {
    use super::*;

    impl<T> Decodable for ::alloc::boxed::Box<T>
    where
        T: Decodable + Sized,
    {
        fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
            T::decode(buf).map(::alloc::boxed::Box::new)
        }
    }

    impl<T> Decodable for ::alloc::sync::Arc<T>
    where
        T: Decodable + Sized,
    {
        fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
            T::decode(buf).map(::alloc::sync::Arc::new)
        }
    }

    impl Decodable for ::alloc::string::String {
        fn decode(from: &mut &[u8]) -> Result<Self, DecodeError> {
            let h = Header::decode(from)?;
            if h.list {
                return Err(DecodeError::UnexpectedList)
            }
            let mut to = ::alloc::vec::Vec::with_capacity(h.payload_length);
            to.extend_from_slice(&from[..h.payload_length]);
            from.advance(h.payload_length);

            Self::from_utf8(to).map_err(|_| DecodeError::Custom("invalid string"))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Overflow,
    LeadingZero,
    InputTooShort,
    NonCanonicalSingleByte,
    NonCanonicalSize,
    UnexpectedLength,
    UnexpectedString,
    UnexpectedList,
    ListLengthMismatch { expected: usize, got: usize },
    Custom(&'static str),
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::Overflow => write!(f, "overflow"),
            DecodeError::LeadingZero => write!(f, "leading zero"),
            DecodeError::InputTooShort => write!(f, "input too short"),
            DecodeError::NonCanonicalSingleByte => write!(f, "non-canonical single byte"),
            DecodeError::NonCanonicalSize => write!(f, "non-canonical size"),
            DecodeError::UnexpectedLength => write!(f, "unexpected length"),
            DecodeError::UnexpectedString => write!(f, "unexpected string"),
            DecodeError::UnexpectedList => write!(f, "unexpected list"),
            DecodeError::ListLengthMismatch { expected, got } => {
                write!(f, "list length mismatch: expected {expected}, got {got}")
            }
            DecodeError::Custom(err) => write!(f, "{err}"),
        }
    }
}

impl Header {
    /// Returns the decoded header.
    ///
    /// Returns an error if the given `buf`'s len is less than the expected payload.
    pub fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        if !buf.has_remaining() {
            return Err(DecodeError::InputTooShort)
        }

        let b = buf[0];
        let h: Self = {
            if b < 0x80 {
                Self { list: false, payload_length: 1 }
            } else if b < 0xB8 {
                buf.advance(1);
                let h = Self { list: false, payload_length: b as usize - 0x80 };

                if h.payload_length == 1 {
                    if !buf.has_remaining() {
                        return Err(DecodeError::InputTooShort)
                    }
                    if buf[0] < 0x80 {
                        return Err(DecodeError::NonCanonicalSingleByte)
                    }
                }

                h
            } else if b < 0xC0 {
                buf.advance(1);
                let len_of_len = b as usize - 0xB7;
                if buf.len() < len_of_len {
                    return Err(DecodeError::InputTooShort)
                }
                let payload_length = usize::try_from(u64::from_be_bytes(
                    static_left_pad(&buf[..len_of_len]).ok_or(DecodeError::LeadingZero)?,
                ))
                .map_err(|_| DecodeError::Custom("Input too big"))?;
                buf.advance(len_of_len);
                if payload_length < 56 {
                    return Err(DecodeError::NonCanonicalSize)
                }

                Self { list: false, payload_length }
            } else if b < 0xF8 {
                buf.advance(1);
                Self { list: true, payload_length: b as usize - 0xC0 }
            } else {
                buf.advance(1);
                let list = true;
                let len_of_len = b as usize - 0xF7;
                if buf.len() < len_of_len {
                    return Err(DecodeError::InputTooShort)
                }
                let payload_length = usize::try_from(u64::from_be_bytes(
                    static_left_pad(&buf[..len_of_len]).ok_or(DecodeError::LeadingZero)?,
                ))
                .map_err(|_| DecodeError::Custom("Input too big"))?;
                buf.advance(len_of_len);
                if payload_length < 56 {
                    return Err(DecodeError::NonCanonicalSize)
                }

                Self { list, payload_length }
            }
        };

        if buf.remaining() < h.payload_length {
            return Err(DecodeError::InputTooShort)
        }

        Ok(h)
    }
}

fn static_left_pad<const LEN: usize>(data: &[u8]) -> Option<[u8; LEN]> {
    if data.len() > LEN {
        return None
    }

    let mut v = [0; LEN];

    if data.is_empty() {
        return Some(v)
    }

    if data[0] == 0 {
        return None
    }

    v[LEN - data.len()..].copy_from_slice(data);
    Some(v)
}

macro_rules! decode_integer {
    ($t:ty) => {
        impl Decodable for $t {
            fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
                let h = Header::decode(buf)?;
                if h.list {
                    return Err(DecodeError::UnexpectedList)
                }
                if h.payload_length > (<$t>::BITS as usize / 8) {
                    return Err(DecodeError::Overflow)
                }
                if buf.remaining() < h.payload_length {
                    return Err(DecodeError::InputTooShort)
                }
                // In the case of 0x80, the Header will be decoded, leaving h.payload_length to be
                // zero.
                // 0x80 is the canonical encoding of 0, so we return 0 here.
                if h.payload_length == 0 {
                    return Ok(<$t>::from(0u8))
                }
                let v = <$t>::from_be_bytes(
                    static_left_pad(&buf[..h.payload_length]).ok_or(DecodeError::LeadingZero)?,
                );
                buf.advance(h.payload_length);
                Ok(v)
            }
        }
    };
}

decode_integer!(usize);
decode_integer!(u8);
decode_integer!(u16);
decode_integer!(u32);
decode_integer!(u64);
decode_integer!(u128);

impl Decodable for bool {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        Ok(match u8::decode(buf)? {
            0 => false,
            1 => true,
            _ => return Err(DecodeError::Custom("invalid bool value, must be 0 or 1")),
        })
    }
}

#[cfg(feature = "std")]
impl Decodable for std::net::IpAddr {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        let h = Header::decode(buf)?;
        if h.list {
            return Err(DecodeError::UnexpectedList)
        }
        let o = match h.payload_length {
            4 => {
                let mut to = [0_u8; 4];
                to.copy_from_slice(&buf[..4]);
                IpAddr::V4(Ipv4Addr::from(to))
            }
            16 => {
                let mut to = [0u8; 16];
                to.copy_from_slice(&buf[..16]);
                IpAddr::V6(Ipv6Addr::from(to))
            }
            _ => return Err(DecodeError::UnexpectedLength),
        };
        buf.advance(h.payload_length);
        Ok(o)
    }
}

#[cfg(feature = "ethnum")]
decode_integer!(ethnum::U256);

#[cfg(feature = "ethereum-types")]
mod ethereum_types_support {
    use super::*;
    use ethereum_types::*;
    use revm_primitives::{ruint::aliases::U128 as RU128, B160, B256, U256 as RU256};

    macro_rules! fixed_hash_impl {
        ($t:ty) => {
            impl Decodable for $t {
                fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
                    Decodable::decode(buf).map(Self)
                }
            }
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
            impl Decodable for $t {
                fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
                    let h = Header::decode(buf)?;
                    if h.list {
                        return Err(DecodeError::UnexpectedList)
                    }
                    if h.payload_length > $n_bytes {
                        return Err(DecodeError::Overflow)
                    }
                    if buf.remaining() < h.payload_length {
                        return Err(DecodeError::InputTooShort)
                    }
                    // In the case of 0x80, the Header will be decoded, leaving h.payload_length to
                    // be zero.
                    // 0x80 is the canonical encoding of 0, so we return 0 here.
                    if h.payload_length == 0 {
                        return Ok(<$t>::from(0u8))
                    }
                    let n = <$t>::from_big_endian(
                        &static_left_pad::<$n_bytes>(&buf[..h.payload_length])
                            .ok_or(DecodeError::LeadingZero)?,
                    );
                    buf.advance(h.payload_length);
                    Ok(n)
                }
            }
        };
    }

    macro_rules! fixed_revm_uint_impl {
        ($t:ty, $n_bytes:tt) => {
            impl Decodable for $t {
                fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
                    let h = Header::decode(buf)?;
                    if h.list {
                        return Err(DecodeError::UnexpectedList)
                    }
                    if h.payload_length > $n_bytes {
                        return Err(DecodeError::Overflow)
                    }
                    if buf.remaining() < h.payload_length {
                        return Err(DecodeError::InputTooShort)
                    }
                    // In the case of 0x80, the Header will be decoded, leaving h.payload_length to
                    // be zero.
                    // 0x80 is the canonical encoding of 0, so we return 0 here.
                    if h.payload_length == 0 {
                        return Ok(<$t>::from(0u8))
                    }
                    let n = <$t>::from_be_bytes(
                        static_left_pad::<$n_bytes>(&buf[..h.payload_length])
                            .ok_or(DecodeError::LeadingZero)?,
                    );
                    buf.advance(h.payload_length);
                    Ok(n)
                }
            }
        };
    }

    fixed_revm_uint_impl!(RU256, 32);
    fixed_revm_uint_impl!(RU128, 16);

    fixed_uint_impl!(U64, 8);
    fixed_uint_impl!(U128, 16);
    fixed_uint_impl!(U256, 32);
    fixed_uint_impl!(U512, 64);
}

impl<const N: usize> Decodable for [u8; N] {
    fn decode(from: &mut &[u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(from)?;
        if h.list {
            return Err(DecodeError::UnexpectedList)
        }
        if h.payload_length != N {
            return Err(DecodeError::UnexpectedLength)
        }

        let mut to = [0_u8; N];
        to.copy_from_slice(&from[..N]);
        from.advance(N);

        Ok(to)
    }
}

impl Decodable for BytesMut {
    fn decode(from: &mut &[u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(from)?;
        if h.list {
            return Err(DecodeError::UnexpectedList)
        }
        let mut to = BytesMut::with_capacity(h.payload_length);
        to.extend_from_slice(&from[..h.payload_length]);
        from.advance(h.payload_length);

        Ok(to)
    }
}

impl Decodable for Bytes {
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        BytesMut::decode(buf).map(BytesMut::freeze)
    }
}

pub struct Rlp<'a> {
    payload_view: &'a [u8],
}

impl<'a> Rlp<'a> {
    pub fn new(mut payload: &'a [u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(&mut payload)?;
        if !h.list {
            return Err(DecodeError::UnexpectedString)
        }

        let payload_view = &payload[..h.payload_length];
        Ok(Self { payload_view })
    }

    pub fn get_next<T: Decodable>(&mut self) -> Result<Option<T>, DecodeError> {
        if self.payload_view.is_empty() {
            return Ok(None)
        }

        Ok(Some(T::decode(&mut self.payload_view)?))
    }
}

#[cfg(feature = "alloc")]
impl<E> Decodable for alloc::vec::Vec<E>
where
    E: Decodable,
{
    fn decode(buf: &mut &[u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(buf)?;
        if !h.list {
            return Err(DecodeError::UnexpectedString)
        }

        let payload_view = &mut &buf[..h.payload_length];

        let mut to = alloc::vec::Vec::new();
        while !payload_view.is_empty() {
            to.push(E::decode(payload_view)?);
        }

        buf.advance(h.payload_length);

        Ok(to)
    }
}

#[cfg(feature = "smol_str")]
impl Decodable for smol_str::SmolStr {
    fn decode(from: &mut &[u8]) -> Result<Self, DecodeError> {
        let h = Header::decode(from)?;
        if h.list {
            return Err(DecodeError::UnexpectedList)
        }
        let data = &from[..h.payload_length];
        let s = match core::str::from_utf8(data) {
            Ok(s) => Ok(smol_str::SmolStr::from(s)),
            Err(_) => Err(DecodeError::Custom("invalid string")),
        };
        from.advance(h.payload_length);
        s
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use crate::Encodable;
    use alloc::vec;
    use core::fmt::Debug;
    use ethereum_types::{U128, U256, U512, U64};
    use ethnum::AsU256;
    use hex_literal::hex;

    fn check_decode<'a, T, IT>(fixtures: IT)
    where
        T: Decodable + PartialEq + Debug,
        IT: IntoIterator<Item = (Result<T, DecodeError>, &'a [u8])>,
    {
        for (expected, mut input) in fixtures {
            assert_eq!(T::decode(&mut input), expected);
            if expected.is_ok() {
                assert_eq!(input, &[]);
            }
        }
    }

    fn check_decode_list<T, IT>(fixtures: IT)
    where
        T: Decodable + PartialEq + Debug,
        IT: IntoIterator<Item = (Result<alloc::vec::Vec<T>, DecodeError>, &'static [u8])>,
    {
        for (expected, mut input) in fixtures {
            assert_eq!(vec::Vec::<T>::decode(&mut input), expected);
            if expected.is_ok() {
                assert_eq!(input, &[]);
            }
        }
    }

    #[test]
    fn rlp_strings() {
        check_decode::<Bytes, _>(vec![
            (Ok(hex!("00")[..].to_vec().into()), &hex!("00")[..]),
            (
                Ok(hex!("6f62636465666768696a6b6c6d")[..].to_vec().into()),
                &hex!("8D6F62636465666768696A6B6C6D")[..],
            ),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
        ])
    }

    #[test]
    fn rlp_fixed_length() {
        check_decode(vec![
            (Ok(hex!("6f62636465666768696a6b6c6d")), &hex!("8D6F62636465666768696A6B6C6D")[..]),
            (Err(DecodeError::UnexpectedLength), &hex!("8C6F62636465666768696A6B6C")[..]),
            (Err(DecodeError::UnexpectedLength), &hex!("8E6F62636465666768696A6B6C6D6E")[..]),
        ])
    }

    #[test]
    fn rlp_u64() {
        check_decode(vec![
            (Ok(9_u64), &hex!("09")[..]),
            (Ok(0_u64), &hex!("80")[..]),
            (Ok(0x0505_u64), &hex!("820505")[..]),
            (Ok(0xCE05050505_u64), &hex!("85CE05050505")[..]),
            (Err(DecodeError::Overflow), &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::InputTooShort), &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Err(DecodeError::Overflow),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[test]
    fn rlp_u256() {
        check_decode(vec![
            (Ok(9_u8.as_u256()), &hex!("09")[..]),
            (Ok(0_u8.as_u256()), &hex!("80")[..]),
            (Ok(0x0505_u16.as_u256()), &hex!("820505")[..]),
            (Ok(0xCE05050505_u64.as_u256()), &hex!("85CE05050505")[..]),
            (Ok(0xFFFFFFFFFFFFFFFFFF7C_u128.as_u256()), &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::InputTooShort), &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Err(DecodeError::Overflow),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[cfg(feature = "ethereum-types")]
    #[test]
    fn rlp_ethereum_types_u64() {
        check_decode(vec![
            (Ok(U64::from(9_u8)), &hex!("09")[..]),
            (Ok(U64::from(0_u8)), &hex!("80")[..]),
            (Ok(U64::from(0x0505_u16)), &hex!("820505")[..]),
            (Ok(U64::from(0xCE05050505_u64)), &hex!("85CE05050505")[..]),
            (Err(DecodeError::Overflow), &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::InputTooShort), &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Err(DecodeError::Overflow),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[cfg(feature = "ethereum-types")]
    #[test]
    fn rlp_ethereum_types_u128() {
        check_decode(vec![
            (Ok(U128::from(9_u8)), &hex!("09")[..]),
            (Ok(U128::from(0_u8)), &hex!("80")[..]),
            (Ok(U128::from(0x0505_u16)), &hex!("820505")[..]),
            (Ok(U128::from(0xCE05050505_u64)), &hex!("85CE05050505")[..]),
            (Ok(U128::from(0xFFFFFFFFFFFFFFFFFF7C_u128)), &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::InputTooShort), &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Err(DecodeError::Overflow),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[cfg(feature = "ethereum-types")]
    #[test]
    fn rlp_ethereum_types_u256() {
        check_decode(vec![
            (Ok(U256::from(9_u8)), &hex!("09")[..]),
            (Ok(U256::from(0_u8)), &hex!("80")[..]),
            (Ok(U256::from(0x0505_u16)), &hex!("820505")[..]),
            (Ok(U256::from(0xCE05050505_u64)), &hex!("85CE05050505")[..]),
            (Ok(U256::from(0xFFFFFFFFFFFFFFFFFF7C_u128)), &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::InputTooShort), &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..]),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Err(DecodeError::Overflow),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[cfg(feature = "ethereum-types")]
    #[test]
    fn rlp_ethereum_types_u512() {
        check_decode(vec![
            (Ok(U512::from(9_u8)), &hex!("09")[..]),
            (Ok(U512::from(0_u8)), &hex!("80")[..]),
            (Ok(U512::from(0x0505_u16)), &hex!("820505")[..]),
            (Ok(U512::from(0xCE05050505_u64)), &hex!("85CE05050505")[..]),
            (
                Ok(U512::from(0xFFFFFFFFFFFFFFFFFF7C_u128)),
                &hex!("8AFFFFFFFFFFFFFFFFFF7C")[..],
            ),
            (
                Err(DecodeError::InputTooShort),
                &hex!("8BFFFFFFFFFFFFFFFFFF7C")[..],
            ),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
            (Err(DecodeError::LeadingZero), &hex!("00")[..]),
            (Err(DecodeError::NonCanonicalSingleByte), &hex!("8105")[..]),
            (Err(DecodeError::LeadingZero), &hex!("8200F4")[..]),
            (Err(DecodeError::NonCanonicalSize), &hex!("B8020004")[..]),
            (
                Ok(U512::from_dec_str("115792089237316195423570985008687907853269984676653278628940326933415738736640").unwrap()),
                &hex!("A101000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
            (
                Err(DecodeError::Overflow),
                &hex!("B84101000000000000000000000000000000000000008B000000000000000000000000000000000000000000000000000000000000008B000000000000000000000000")[..],
            ),
        ])
    }

    #[test]
    fn rlp_vectors() {
        check_decode_list(vec![
            (Ok(vec![]), &hex!("C0")[..]),
            (Ok(vec![0xBBCCB5_u64, 0xFFC0B5_u64]), &hex!("C883BBCCB583FFC0B5")[..]),
        ])
    }

    #[cfg(feature = "smol_str")]
    #[test]
    fn rlp_smol_str() {
        use smol_str::SmolStr;
        let mut b = BytesMut::new();
        "test smol str".to_string().encode(&mut b);
        check_decode::<SmolStr, _>(vec![
            (Ok(SmolStr::new("test smol str")), b.as_ref()),
            (Err(DecodeError::UnexpectedList), &hex!("C0")[..]),
        ])
    }
}
