//! Scroll transaction type.

pub use scroll_alloy_consensus::ScrollTxType;

#[cfg(test)]
mod tests {
    use super::*;
    use reth_codecs::{txtype::*, Compact};
    use rstest::rstest;
    use scroll_alloy_consensus::L1_MESSAGE_TX_TYPE_ID;

    #[rstest]
    #[case(ScrollTxType::Legacy, COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(ScrollTxType::Eip2930, COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(ScrollTxType::Eip1559, COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(ScrollTxType::L1Message, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![L1_MESSAGE_TX_TYPE_ID])]
    fn test_txtype_to_compact(
        #[case] tx_type: ScrollTxType,
        #[case] expected_identifier: usize,
        #[case] expected_buf: Vec<u8>,
    ) {
        let mut buf = vec![];
        let identifier = tx_type.to_compact(&mut buf);

        assert_eq!(
            identifier, expected_identifier,
            "Unexpected identifier for ScrollTxType {tx_type:?}",
        );
        assert_eq!(buf, expected_buf, "Unexpected buffer for ScrollTxType {tx_type:?}",);
    }

    #[rstest]
    #[case(ScrollTxType::Legacy, COMPACT_IDENTIFIER_LEGACY, vec![])]
    #[case(ScrollTxType::Eip2930, COMPACT_IDENTIFIER_EIP2930, vec![])]
    #[case(ScrollTxType::Eip1559, COMPACT_IDENTIFIER_EIP1559, vec![])]
    #[case(ScrollTxType::L1Message, COMPACT_EXTENDED_IDENTIFIER_FLAG, vec![L1_MESSAGE_TX_TYPE_ID])]
    fn test_txtype_from_compact(
        #[case] expected_type: ScrollTxType,
        #[case] identifier: usize,
        #[case] buf: Vec<u8>,
    ) {
        let (actual_type, remaining_buf) = ScrollTxType::from_compact(&buf, identifier);

        assert_eq!(actual_type, expected_type, "Unexpected TxType for identifier {identifier}");
        assert!(remaining_buf.is_empty(), "Buffer not fully consumed for identifier {identifier}");
    }
}
