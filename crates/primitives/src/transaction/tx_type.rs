use alloy_consensus::constants::{
    EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};
use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable};
use derive_more::Display;
use reth_primitives_traits::InMemorySize;
use serde::{Deserialize, Serialize};

/// Transaction Type
///
/// Currently being used as 2-bit type when encoding it to `reth_codecs::Compact` on
/// [`crate::TransactionSignedNoHash`]. Adding more transaction types will break the codec and
/// database format.
///
/// Other required changes when adding a new type can be seen on [PR#3953](https://github.com/paradigmxyz/reth/pull/3953/files).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Serialize,
    Deserialize,
    Hash,
    Display,
)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(compact))]
#[display("tx type: {_variant}")]
pub enum TxType {
    /// Legacy transaction pre EIP-2929
    #[default]
    #[display("legacy (0)")]
    Legacy = 0_isize,
    /// AccessList transaction
    #[display("eip2930 (1)")]
    Eip2930 = 1_isize,
    /// Transaction with Priority fee
    #[display("eip1559 (2)")]
    Eip1559 = 2_isize,
    /// Shard Blob Transactions - EIP-4844
    #[display("eip4844 (3)")]
    Eip4844 = 3_isize,
    /// EOA Contract Code Transactions - EIP-7702
    #[display("eip7702 (4)")]
    Eip7702 = 4_isize,
    /// Optimism Deposit transaction.
    #[cfg(feature = "optimism")]
    #[display("deposit (126)")]
    Deposit = 126_isize,
}

impl TxType {
    /// The max type reserved by an EIP.
    pub const MAX_RESERVED_EIP: Self = Self::Eip7702;

    /// Check if the transaction type has an access list.
    pub const fn has_access_list(&self) -> bool {
        match self {
            Self::Legacy => false,
            Self::Eip2930 | Self::Eip1559 | Self::Eip4844 | Self::Eip7702 => true,
            #[cfg(feature = "optimism")]
            Self::Deposit => false,
        }
    }
}

impl reth_primitives_traits::TxType for TxType {
    #[inline]
    fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }

    #[inline]
    fn is_eip2930(&self) -> bool {
        matches!(self, Self::Eip2930)
    }

    #[inline]
    fn is_eip1559(&self) -> bool {
        matches!(self, Self::Eip1559)
    }

    #[inline]
    fn is_eip4844(&self) -> bool {
        matches!(self, Self::Eip4844)
    }

    #[inline]
    fn is_eip7702(&self) -> bool {
        matches!(self, Self::Eip7702)
    }
}

impl InMemorySize for TxType {
    /// Calculates a heuristic for the in-memory size of the [`TxType`].
    #[inline]
    fn size(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

impl From<TxType> for u8 {
    fn from(value: TxType) -> Self {
        match value {
            TxType::Legacy => LEGACY_TX_TYPE_ID,
            TxType::Eip2930 => EIP2930_TX_TYPE_ID,
            TxType::Eip1559 => EIP1559_TX_TYPE_ID,
            TxType::Eip4844 => EIP4844_TX_TYPE_ID,
            TxType::Eip7702 => EIP7702_TX_TYPE_ID,
            #[cfg(feature = "optimism")]
            TxType::Deposit => op_alloy_consensus::DEPOSIT_TX_TYPE_ID,
        }
    }
}

impl From<TxType> for U8 {
    fn from(value: TxType) -> Self {
        Self::from(u8::from(value))
    }
}

impl TryFrom<u8> for TxType {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        #[cfg(feature = "optimism")]
        if value == Self::Deposit {
            return Ok(Self::Deposit)
        }

        if value == Self::Legacy {
            return Ok(Self::Legacy)
        } else if value == Self::Eip2930 {
            return Ok(Self::Eip2930)
        } else if value == Self::Eip1559 {
            return Ok(Self::Eip1559)
        } else if value == Self::Eip4844 {
            return Ok(Self::Eip4844)
        } else if value == Self::Eip7702 {
            return Ok(Self::Eip7702)
        }

        Err("invalid tx type")
    }
}

impl TryFrom<u64> for TxType {
    type Error = &'static str;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let value: u8 = value.try_into().map_err(|_| "invalid tx type")?;
        Self::try_from(value)
    }
}

impl TryFrom<U64> for TxType {
    type Error = &'static str;

    fn try_from(value: U64) -> Result<Self, Self::Error> {
        value.to::<u64>().try_into()
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for TxType {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use reth_codecs::txtype::*;

        match self {
            Self::Legacy => COMPACT_IDENTIFIER_LEGACY,
            Self::Eip2930 => COMPACT_IDENTIFIER_EIP2930,
            Self::Eip1559 => COMPACT_IDENTIFIER_EIP1559,
            Self::Eip4844 => {
                buf.put_u8(EIP4844_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
            Self::Eip7702 => {
                buf.put_u8(EIP7702_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
            #[cfg(feature = "optimism")]
            Self::Deposit => {
                buf.put_u8(op_alloy_consensus::DEPOSIT_TX_TYPE_ID);
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
                reth_codecs::txtype::COMPACT_IDENTIFIER_LEGACY => Self::Legacy,
                reth_codecs::txtype::COMPACT_IDENTIFIER_EIP2930 => Self::Eip2930,
                reth_codecs::txtype::COMPACT_IDENTIFIER_EIP1559 => Self::Eip1559,
                reth_codecs::txtype::COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        EIP4844_TX_TYPE_ID => Self::Eip4844,
                        EIP7702_TX_TYPE_ID => Self::Eip7702,
                        #[cfg(feature = "optimism")]
                        op_alloy_consensus::DEPOSIT_TX_TYPE_ID => Self::Deposit,
                        _ => panic!("Unsupported TxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}

impl PartialEq<u8> for TxType {
    fn eq(&self, other: &u8) -> bool {
        *self as u8 == *other
    }
}

impl PartialEq<TxType> for u8 {
    fn eq(&self, other: &TxType) -> bool {
        *self == *other as Self
    }
}

impl Encodable for TxType {
    fn encode(&self, out: &mut dyn bytes::BufMut) {
        (*self as u8).encode(out);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for TxType {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let ty = u8::decode(buf)?;

        Self::try_from(ty).map_err(alloy_rlp::Error::Custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::hex;
    use reth_codecs::{txtype::*, Compact};
    use rstest::rstest;

    #[rstest]
    #[case(U64::from(LEGACY_TX_TYPE_ID), Ok(TxType::Legacy))]
    #[case(U64::from(EIP2930_TX_TYPE_ID), Ok(TxType::Eip2930))]
    #[case(U64::from(EIP1559_TX_TYPE_ID), Ok(TxType::Eip1559))]
    #[case(U64::from(EIP4844_TX_TYPE_ID), Ok(TxType::Eip4844))]
    #[case(U64::from(EIP7702_TX_TYPE_ID), Ok(TxType::Eip7702))]
    #[cfg_attr(
        feature = "optimism",
        case(U64::from(op_alloy_consensus::DEPOSIT_TX_TYPE_ID), Ok(TxType::Deposit))
    )]
    #[case(U64::MAX, Err("invalid tx type"))]
    fn test_u64_to_tx_type(#[case] input: U64, #[case] expected: Result<TxType, &'static str>) {
        let tx_type_result = TxType::try_from(input);
        assert_eq!(tx_type_result, expected);
    }

    #[rstest]
    #[case(TxType::Legacy, COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(TxType::Eip2930, COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(TxType::Eip1559, COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(TxType::Eip4844, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP4844_TX_TYPE_ID])]
    #[case(TxType::Eip7702, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP7702_TX_TYPE_ID])]
    #[cfg_attr(feature = "optimism", case(TxType::Deposit, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![op_alloy_consensus::DEPOSIT_TX_TYPE_ID]))]
    fn test_txtype_to_compact(
        #[case] tx_type: TxType,
        #[case] expected_identifier: usize,
        #[case] expected_buf: Vec<u8>,
    ) {
        let mut buf = vec![];
        let identifier = tx_type.to_compact(&mut buf);

        assert_eq!(identifier, expected_identifier, "Unexpected identifier for TxType {tx_type:?}",);
        assert_eq!(buf, expected_buf, "Unexpected buffer for TxType {tx_type:?}",);
    }

    #[rstest]
    #[case(TxType::Legacy, COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(TxType::Eip2930, COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(TxType::Eip1559, COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(TxType::Eip4844, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP4844_TX_TYPE_ID])]
    #[case(TxType::Eip7702, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP7702_TX_TYPE_ID])]
    #[cfg_attr(feature = "optimism", case(TxType::Deposit, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![op_alloy_consensus::DEPOSIT_TX_TYPE_ID]))]
    fn test_txtype_from_compact(
        #[case] expected_type: TxType,
        #[case] identifier: usize,
        #[case] buf: Vec<u8>,
    ) {
        let (actual_type, remaining_buf) = TxType::from_compact(&buf, identifier);

        assert_eq!(actual_type, expected_type, "Unexpected TxType for identifier {identifier}");
        assert!(remaining_buf.is_empty(), "Buffer not fully consumed for identifier {identifier}");
    }

    #[rstest]
    #[case(&hex!("80"), Ok(TxType::Legacy))]
    #[case(&[EIP2930_TX_TYPE_ID], Ok(TxType::Eip2930))]
    #[case(&[EIP1559_TX_TYPE_ID], Ok(TxType::Eip1559))]
    #[case(&[EIP4844_TX_TYPE_ID], Ok(TxType::Eip4844))]
    #[case(&[EIP7702_TX_TYPE_ID], Ok(TxType::Eip7702))]
    #[case(&[u8::MAX], Err(alloy_rlp::Error::InputTooShort))]
    #[cfg_attr(feature = "optimism", case(&[op_alloy_consensus::DEPOSIT_TX_TYPE_ID], Ok(TxType::Deposit)))]
    fn decode_tx_type(#[case] input: &[u8], #[case] expected: Result<TxType, alloy_rlp::Error>) {
        let tx_type_result = TxType::decode(&mut &input[..]);
        assert_eq!(tx_type_result, expected)
    }
}
