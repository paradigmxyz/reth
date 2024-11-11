//! newtype pattern on op_alloy_consensus::OpTxType.
//! OpTxType that implements reth_primitives_traits::TxType.
//! This type is required because a Compact impl is needed on the deposit tx type.

use alloy_eips::eip2718::Eip2718Error;
use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use core::fmt::Debug;
use derive_more::{
    derive::{From, Into},
    Display,
};
use op_alloy_consensus::OpTxType as AlloyOpTxType;
use reth_primitives_traits::TxType;
use std::convert::TryFrom;

/// Wrapper type for `AlloyOpTxType` to implement `TxType` trait.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Display, Ord, Hash, From, Into)]
#[from(AlloyOpTxType)]
#[into(u8)]
pub struct OpTxType(AlloyOpTxType);

impl From<OpTxType> for U8 {
    fn from(tx_type: OpTxType) -> Self {
        U8::from(u8::from(tx_type))
    }
}

impl TryFrom<u8> for OpTxType {
    type Error = Eip2718Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        AlloyOpTxType::try_from(value)
            .map(OpTxType)
            .map_err(|_| Eip2718Error::UnexpectedType(value))
    }
}

impl Default for OpTxType {
    fn default() -> Self {
        OpTxType(AlloyOpTxType::Legacy)
    }
}

impl PartialEq<u8> for OpTxType {
    fn eq(&self, other: &u8) -> bool {
        let self_as_u8: u8 = (*self).into();
        &self_as_u8 == other
    }
}

impl TryFrom<u64> for OpTxType {
    type Error = Eip2718Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > u8::MAX as u64 {
            return Err(Eip2718Error::UnexpectedType(0));
        }
        Self::try_from(value as u8)
    }
}

impl TryFrom<U64> for OpTxType {
    type Error = Eip2718Error;

    fn try_from(value: U64) -> Result<Self, Self::Error> {
        let u64_value: u64 = value.try_into().map_err(|_| Eip2718Error::UnexpectedType(0))?;
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

        OpTxType::try_from(value).map_err(|_| alloy_rlp::Error::Custom("Invalid transaction type"))
    }
}

impl TxType for OpTxType {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip2718::Eip2718Error;
    use alloy_primitives::{U64, U8};
    use alloy_rlp::{Decodable, Encodable};
    use bytes::BytesMut;
    use op_alloy_consensus::OpTxType as AlloyOpTxType;

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
        assert!(matches!(result, Err(Eip2718Error::UnexpectedType(v)) if v == invalid_value));
    }

    #[test]
    fn test_try_from_u64() {
        let op_tx = OpTxType::try_from(AlloyOpTxType::Legacy as u64).unwrap();
        assert_eq!(op_tx, OpTxType(AlloyOpTxType::Legacy));
    }

    #[test]
    fn test_try_from_u64_out_of_range() {
        let result = OpTxType::try_from(u64::MAX);
        assert!(matches!(result, Err(Eip2718Error::UnexpectedType(0))));
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
}
