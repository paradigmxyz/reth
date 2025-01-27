//! Compact implementation for [`TxType`]

use crate::txtype::{COMPACT_EXTENDED_IDENTIFIER_FLAG, COMPACT_IDENTIFIER_EIP1559, COMPACT_IDENTIFIER_EIP2930, COMPACT_IDENTIFIER_LEGACY};
use alloy_consensus::constants::{EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID};
use alloy_consensus::TxType;

impl crate::Compact for TxType {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        use crate::txtype::*;

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
                        EIP4844_TX_TYPE_ID => Self::Eip4844,
                        EIP7702_TX_TYPE_ID => Self::Eip7702,
                        _ => panic!("Unsupported TxType identifier: {extended_identifier}"),
                    }
                }
                _ => panic!("Unknown identifier for TxType: {identifier}"),
            },
            buf,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    
    use alloy_consensus::constants::{EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID};
    use crate::Compact;


    #[rstest]
    #[case(TxType::Legacy, COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(TxType::Eip2930, COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(TxType::Eip1559, COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(TxType::Eip4844, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP4844_TX_TYPE_ID])]
    #[case(TxType::Eip7702, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![EIP7702_TX_TYPE_ID])]
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
    fn test_txtype_from_compact(
        #[case] expected_type: TxType,
        #[case] identifier: usize,
        #[case] buf: Vec<u8>,
    ) {
        let (actual_type, remaining_buf) = TxType::from_compact(&buf, identifier);

        assert_eq!(actual_type, expected_type, "Unexpected TxType for identifier {identifier}");
        assert!(remaining_buf.is_empty(), "Buffer not fully consumed for identifier {identifier}");
    }
}