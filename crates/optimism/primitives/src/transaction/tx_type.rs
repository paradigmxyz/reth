//! newtype pattern on `op_alloy_consensus::OpTxType`.
//! `OpTxType` implements `reth_primitives_traits::TxType`.
//! This type is required because a `Compact` impl is needed on the deposit tx type.

use core::fmt::Debug;

use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable, Error};
use bytes::BufMut;
use derive_more::{
    derive::{From, Into},
    Display,
};
use op_alloy_consensus::OpTxType as AlloyOpTxType;
use reth_primitives_traits::{InMemorySize, TxType};

/// Wrapper type for [`op_alloy_consensus::OpTxType`] to implement
/// [`TxType`] trait.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Display, Ord, Hash, From, Into)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[into(u8)]
pub struct OpTxType(AlloyOpTxType);

impl TxType for OpTxType {
    #[inline]
    fn is_legacy(&self) -> bool {
        matches!(self.0, AlloyOpTxType::Legacy)
    }

    #[inline]
    fn is_eip2930(&self) -> bool {
        matches!(self.0, AlloyOpTxType::Eip2930)
    }

    #[inline]
    fn is_eip1559(&self) -> bool {
        matches!(self.0, AlloyOpTxType::Eip1559)
    }

    #[inline]
    fn is_eip4844(&self) -> bool {
        false
    }

    #[inline]
    fn is_eip7702(&self) -> bool {
        matches!(self.0, AlloyOpTxType::Eip7702)
    }
}

impl InMemorySize for OpTxType {
    /// Calculates a heuristic for the in-memory size of the [`OpTxType`].
    #[inline]
    fn size(&self) -> usize {
        core::mem::size_of::<Self>()
    }
}

impl From<OpTxType> for U8 {
    fn from(tx_type: OpTxType) -> Self {
        Self::from(u8::from(tx_type))
    }
}

impl TryFrom<u8> for OpTxType {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        AlloyOpTxType::try_from(value)
            .map(OpTxType)
            .map_err(|_| Error::Custom("Invalid transaction type"))
    }
}

impl Default for OpTxType {
    fn default() -> Self {
        Self(AlloyOpTxType::Legacy)
    }
}

impl PartialEq<u8> for OpTxType {
    fn eq(&self, other: &u8) -> bool {
        let self_as_u8: u8 = (*self).into();
        &self_as_u8 == other
    }
}

impl TryFrom<u64> for OpTxType {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > u8::MAX as u64 {
            return Err(Error::Custom("value out of range"));
        }
        Self::try_from(value as u8)
    }
}

impl TryFrom<U64> for OpTxType {
    type Error = Error;

    fn try_from(value: U64) -> Result<Self, Self::Error> {
        let u64_value: u64 = value.try_into().map_err(|_| Error::Custom("value out of range"))?;
        Self::try_from(u64_value)
    }
}

impl Encodable for OpTxType {
    fn length(&self) -> usize {
        let value: u8 = (*self).into();
        value.length()
    }

    fn encode(&self, out: &mut dyn BufMut) {
        let value: u8 = (*self).into();
        value.encode(out);
    }
}

impl Decodable for OpTxType {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        // Decode the u8 value from RLP
        let value = if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort);
        } else if buf[0] == 0x80 {
            0 // Special case: RLP encoding for integer 0 is `b"\x80"`
        } else {
            u8::decode(buf)?
        };

        Self::try_from(value).map_err(|_| alloy_rlp::Error::Custom("Invalid transaction type"))
    }
}

#[cfg(any(test, feature = "reth-codec"))]
impl reth_codecs::Compact for OpTxType {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use reth_codecs::txtype::*;
        match self.0 {
            AlloyOpTxType::Legacy => COMPACT_IDENTIFIER_LEGACY,
            AlloyOpTxType::Eip2930 => COMPACT_IDENTIFIER_EIP2930,
            AlloyOpTxType::Eip1559 => COMPACT_IDENTIFIER_EIP1559,
            AlloyOpTxType::Eip7702 => {
                buf.put_u8(alloy_consensus::constants::EIP7702_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
            AlloyOpTxType::Deposit => {
                buf.put_u8(op_alloy_consensus::DEPOSIT_TX_TYPE_ID);
                COMPACT_EXTENDED_IDENTIFIER_FLAG
            }
        }
    }

    fn from_compact(mut buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        use bytes::Buf;
        (
            match identifier {
                reth_codecs::txtype::COMPACT_IDENTIFIER_LEGACY => Self(AlloyOpTxType::Legacy),
                reth_codecs::txtype::COMPACT_IDENTIFIER_EIP2930 => Self(AlloyOpTxType::Eip2930),
                reth_codecs::txtype::COMPACT_IDENTIFIER_EIP1559 => Self(AlloyOpTxType::Eip1559),
                reth_codecs::txtype::COMPACT_EXTENDED_IDENTIFIER_FLAG => {
                    let extended_identifier = buf.get_u8();
                    match extended_identifier {
                        alloy_consensus::constants::EIP7702_TX_TYPE_ID => {
                            Self(AlloyOpTxType::Eip7702)
                        }
                        op_alloy_consensus::DEPOSIT_TX_TYPE_ID => Self(AlloyOpTxType::Deposit),
                        _ => panic!("Unsupported OpTxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for OpTxType: {identifier}"),
            },
            buf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::constants::EIP7702_TX_TYPE_ID;
    use bytes::BytesMut;
    use op_alloy_consensus::DEPOSIT_TX_TYPE_ID;
    use reth_codecs::{txtype::*, Compact};
    use rstest::rstest;

    #[test]
    fn test_from_alloy_op_tx_type() {
        let alloy_tx = AlloyOpTxType::Legacy;
        let op_tx: OpTxType = OpTxType::from(alloy_tx);
        assert_eq!(op_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_from_op_tx_type_to_u8() {
        let op_tx = OpTxType(AlloyOpTxType::Legacy);
        let tx_type_u8: u8 = op_tx.into();
        assert_eq!(tx_type_u8, AlloyOpTxType::Legacy as u8);
    }

    #[test]
    fn test_from_op_tx_type_to_u8_u8() {
        let op_tx = OpTxType(AlloyOpTxType::Legacy);
        let tx_type_u8: U8 = op_tx.into();
        assert_eq!(tx_type_u8, U8::from(AlloyOpTxType::Legacy as u8));
    }

    #[test]
    fn test_try_from_u8() {
        let op_tx = OpTxType::try_from(AlloyOpTxType::Legacy as u8).unwrap();
        assert_eq!(op_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_try_from_invalid_u8() {
        let invalid_value: u8 = 255;
        let result = OpTxType::try_from(invalid_value);
        assert_eq!(result, Err(Error::Custom("Invalid transaction type")));
    }

    #[test]
    fn test_try_from_u64() {
        let op_tx = OpTxType::try_from(AlloyOpTxType::Legacy as u64).unwrap();
        assert_eq!(op_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_try_from_u64_out_of_range() {
        let result = OpTxType::try_from(u64::MAX);
        assert_eq!(result, Err(Error::Custom("value out of range")));
    }

    #[test]
    fn test_try_from_u64_within_range() {
        let valid_value: U64 = U64::from(AlloyOpTxType::Legacy as u64);
        let op_tx = OpTxType::try_from(valid_value).unwrap();
        assert_eq!(op_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_default() {
        let default_tx = OpTxType::default();
        assert_eq!(default_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_partial_eq_u8() {
        let op_tx = OpTxType(AlloyOpTxType::Legacy);
        assert_eq!(op_tx, AlloyOpTxType::Legacy as u8);
    }

    #[test]
    fn test_encodable() {
        let op_tx = OpTxType(AlloyOpTxType::Legacy);
        let mut buf = BytesMut::new();
        op_tx.encode(&mut buf);
        assert_eq!(buf, BytesMut::from(&[0x80][..]));
    }

    #[test]
    fn test_decodable_success() {
        // Using the RLP-encoded form of 0, which is `b"\x80"`
        let mut buf: &[u8] = &[0x80];
        let decoded_tx = OpTxType::decode(&mut buf).unwrap();
        assert_eq!(decoded_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_decodable_invalid() {
        let mut buf: &[u8] = &[255];
        let result = OpTxType::decode(&mut buf);
        assert!(result.is_err());
    }

    #[rstest]
    #[case(OpTxType(AlloyOpTxType::Legacy), COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip2930), COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip1559), COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip7702), COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP7702_TX_TYPE_ID])]
    #[case(OpTxType(AlloyOpTxType::Deposit), COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![DEPOSIT_TX_TYPE_ID])]
    fn test_txtype_to_compact(
        #[case] tx_type: OpTxType,
        #[case] expected_identifier: usize,
        #[case] expected_buf: Vec<u8>,
    ) {
        let mut buf = vec![];
        let identifier = tx_type.to_compact(&mut buf);

        assert_eq!(
            identifier, expected_identifier,
            "Unexpected identifier for OpTxType {tx_type:?}",
        );
        assert_eq!(buf, expected_buf, "Unexpected buffer for OpTxType {tx_type:?}",);
    }

    #[rstest]
    #[case(OpTxType(AlloyOpTxType::Legacy), COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip2930), COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip1559), COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(OpTxType(AlloyOpTxType::Eip7702), COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP7702_TX_TYPE_ID])]
    #[case(OpTxType(AlloyOpTxType::Deposit), COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![DEPOSIT_TX_TYPE_ID])]
    fn test_txtype_from_compact(
        #[case] expected_type: OpTxType,
        #[case] identifier: usize,
        #[case] buf: Vec<u8>,
    ) {
        let (actual_type, remaining_buf) = OpTxType::from_compact(&buf, identifier);

        assert_eq!(actual_type, expected_type, "Unexpected TxType for identifier {identifier}");
        assert!(remaining_buf.is_empty(), "Buffer not fully consumed for identifier {identifier}");
    }
}
