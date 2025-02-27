//! Scroll-specific payload attributes.

use alloc::vec::Vec;
use alloy_primitives::Bytes;
use alloy_rpc_types_engine::PayloadAttributes;

/// The payload attributes for block building tailored for Scroll.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct ScrollPayloadAttributes {
    /// The payload attributes.
    pub payload_attributes: PayloadAttributes,
    /// An optional array of transaction to be forced included in the block (includes l1 messages).
    pub transactions: Option<Vec<Bytes>>,
    /// Indicates whether the payload building job should happen with or without pool transactions.
    pub no_tx_pool: bool,
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for ScrollPayloadAttributes {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            payload_attributes: PayloadAttributes {
                timestamp: u64::arbitrary(u)?,
                prev_randao: alloy_primitives::B256::arbitrary(u)?,
                suggested_fee_recipient: alloy_primitives::Address::arbitrary(u)?,
                withdrawals: None,
                parent_beacon_block_root: Some(alloy_primitives::B256::arbitrary(u)?),
            },
            transactions: Some(Vec::arbitrary(u)?),
            no_tx_pool: bool::arbitrary(u)?,
        })
    }
}

#[cfg(all(test, feature = "serde"))]
mod test {
    use super::*;
    use alloy_primitives::{Address, B256};
    use alloy_rpc_types_engine::PayloadAttributes;

    #[test]
    fn test_serde_roundtrip_attributes() {
        let attributes = ScrollPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: 0x1337,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: Address::ZERO,
                withdrawals: Default::default(),
                parent_beacon_block_root: Some(B256::ZERO),
            },
            transactions: Some(vec![b"hello".to_vec().into()]),
            no_tx_pool: true,
        };

        let ser = serde_json::to_string(&attributes).unwrap();
        let de: ScrollPayloadAttributes = serde_json::from_str(&ser).unwrap();

        assert_eq!(attributes, de);
    }
}
