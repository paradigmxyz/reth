//! Contains the transaction type identifier for Scroll.

use alloy_consensus::Typed2718;
use alloy_eips::eip2718::Eip2718Error;
use alloy_primitives::{U64, U8};
use alloy_rlp::{BufMut, Decodable, Encodable};
use derive_more::Display;
#[cfg(feature = "reth-codec")]
use reth_codecs::{
    __private::bytes,
    txtype::{
        COMPACT_EXTENDED_IDENTIFIER_FLAG, COMPACT_IDENTIFIER_EIP1559, COMPACT_IDENTIFIER_EIP2930,
        COMPACT_IDENTIFIER_LEGACY,
    },
    Compact,
};

/// Identifier for an Scroll L1 message transaction
pub const L1_MESSAGE_TX_TYPE_ID: u8 = 126; // 0x7E

/// Scroll `TransactionType` flags as specified in <https://docs.scroll.io/en/technology/chain/transactions/>.
#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Hash, Display)]
pub enum ScrollTxType {
    /// Legacy transaction type.
    #[display("legacy")]
    Legacy = 0,
    /// EIP-2930 transaction type.
    #[display("eip2930")]
    Eip2930 = 1,
    /// EIP-1559 transaction type.
    #[display("eip1559")]
    Eip1559 = 2,
    /// EIP-7702 transaction type.
    #[display("eip7702")]
    Eip7702 = 4,
    /// L1 message transaction type.
    #[display("l1_message")]
    L1Message = L1_MESSAGE_TX_TYPE_ID,
}

impl ScrollTxType {
    /// List of all variants.
    pub const ALL: [Self; 5] =
        [Self::Legacy, Self::Eip1559, Self::Eip2930, Self::Eip7702, Self::L1Message];
}

#[cfg(any(test, feature = "arbitrary"))]
impl arbitrary::Arbitrary<'_> for ScrollTxType {
    fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
        let i = u.choose_index(Self::ALL.len())?;
        Ok(Self::ALL[i])
    }
}

impl From<ScrollTxType> for U8 {
    fn from(tx_type: ScrollTxType) -> Self {
        Self::from(u8::from(tx_type))
    }
}

impl From<ScrollTxType> for u8 {
    fn from(v: ScrollTxType) -> Self {
        v as Self
    }
}

impl TryFrom<u8> for ScrollTxType {
    type Error = Eip2718Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            x if x == Self::Legacy as u8 => Self::Legacy,
            x if x == Self::Eip2930 as u8 => Self::Eip2930,
            x if x == Self::Eip1559 as u8 => Self::Eip1559,
            x if x == Self::Eip7702 as u8 => Self::Eip7702,
            x if x == Self::L1Message as u8 => Self::L1Message,
            _ => return Err(Eip2718Error::UnexpectedType(value)),
        })
    }
}

impl TryFrom<u64> for ScrollTxType {
    type Error = &'static str;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let err = || "invalid tx type";
        let value: u8 = value.try_into().map_err(|_| err())?;
        Self::try_from(value).map_err(|_| err())
    }
}

impl TryFrom<U64> for ScrollTxType {
    type Error = &'static str;

    fn try_from(value: U64) -> Result<Self, Self::Error> {
        value.to::<u64>().try_into()
    }
}

impl PartialEq<u8> for ScrollTxType {
    fn eq(&self, other: &u8) -> bool {
        (*self as u8) == *other
    }
}

impl PartialEq<ScrollTxType> for u8 {
    fn eq(&self, other: &ScrollTxType) -> bool {
        *self == *other as Self
    }
}

impl Encodable for ScrollTxType {
    fn encode(&self, out: &mut dyn BufMut) {
        (*self as u8).encode(out);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for ScrollTxType {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let ty = u8::decode(buf)?;

        Self::try_from(ty).map_err(|_| alloy_rlp::Error::Custom("invalid transaction type"))
    }
}

#[cfg(feature = "reth-codec")]
impl Compact for ScrollTxType {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Legacy => COMPACT_IDENTIFIER_LEGACY,
            Self::Eip2930 => COMPACT_IDENTIFIER_EIP2930,
            Self::Eip1559 => COMPACT_IDENTIFIER_EIP1559,
            Self::Eip7702 => {
                buf.put_u8(alloy_eips::eip7702::constants::EIP7702_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
            Self::L1Message => {
                buf.put_u8(L1_MESSAGE_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
        }
    }

    // For backwards compatibility purposes only 2 bits of the type are encoded in the identifier
    // parameter. In the case of a [`COMPACT_EXTENDED_IDENTIFIER_FLAG`], the full transaction type
    // is read from the buffer as a single byte.
    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        (
            match identifier {
                COMPACT_IDENTIFIER_LEGACY => Self::Legacy,
                COMPACT_IDENTIFIER_EIP2930 => Self::Eip2930,
                COMPACT_IDENTIFIER_EIP1559 => Self::Eip1559,
                COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        alloy_eips::eip7702::constants::EIP7702_TX_TYPE_ID => Self::Eip7702,
                        L1_MESSAGE_TX_TYPE_ID => Self::L1Message,
                        _ => panic!("Unsupported TxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}

impl Typed2718 for ScrollTxType {
    fn ty(&self) -> u8 {
        (*self).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::{vec, vec::Vec};

    #[test]
    fn test_all_tx_types() {
        assert_eq!(ScrollTxType::ALL.len(), 5);
        let all = vec![
            ScrollTxType::Legacy,
            ScrollTxType::Eip1559,
            ScrollTxType::Eip2930,
            ScrollTxType::Eip7702,
            ScrollTxType::L1Message,
        ];
        assert_eq!(ScrollTxType::ALL.to_vec(), all);
    }

    #[test]
    fn tx_type_roundtrip() {
        for &tx_type in &ScrollTxType::ALL {
            let mut buf = Vec::new();
            tx_type.encode(&mut buf);
            let decoded = ScrollTxType::decode(&mut &buf[..]).unwrap();
            assert_eq!(tx_type, decoded);
        }
    }
}
