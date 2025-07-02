//! Scroll-specific payload attributes.

use alloc::vec::Vec;
use alloy_primitives::{Address, Bytes, B256, U256};
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
    /// The block data hint, used pre-Euclid by the block builder to derive the correct block
    /// hash and post-Euclid by the sequencer to set the difficulty of the block.
    pub block_data_hint: BlockDataHint,
    /// The gas limit for the block building task.
    pub gas_limit: Option<u64>,
}

/// Block data provided as a hint to the payload attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct BlockDataHint {
    /// The optional extra data for the block.
    pub extra_data: Option<Bytes>,
    /// The optional state root for the block.
    pub state_root: Option<B256>,
    /// The optional coinbase for the block.
    pub coinbase: Option<Address>,
    /// The optional nonce for the block.
    pub nonce: Option<u64>,
    /// The optional difficulty for the block.
    pub difficulty: Option<U256>,
}

impl BlockDataHint {
    /// Returns an empty [`BlockDataHint`] with all fields set to `None`.
    pub fn none() -> Self {
        Self::default()
    }

    /// Returns `true` if the [`BlockDataHint`] is empty.
    pub const fn is_empty(&self) -> bool {
        self.extra_data.is_none() &&
            self.state_root.is_none() &&
            self.coinbase.is_none() &&
            self.nonce.is_none() &&
            self.difficulty.is_none()
    }
}

#[cfg(feature = "arbitrary")]
impl<'a> arbitrary::Arbitrary<'a> for BlockDataHint {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            extra_data: Some(Bytes::arbitrary(u)?),
            state_root: Some(B256::arbitrary(u)?),
            coinbase: Some(Address::arbitrary(u)?),
            nonce: Some(u64::arbitrary(u)?),
            difficulty: Some(U256::arbitrary(u)?),
        })
    }
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
            block_data_hint: BlockDataHint::arbitrary(u)?,
            gas_limit: Some(u64::arbitrary(u)?),
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
            block_data_hint: BlockDataHint {
                extra_data: Some(b"world".into()),
                state_root: Some(B256::random()),
                coinbase: Some(Address::random()),
                nonce: Some(0x12345),
                difficulty: Some(U256::from(10)),
            },
            gas_limit: Some(10_000_000),
        };

        let ser = serde_json::to_string(&attributes).unwrap();
        let de: ScrollPayloadAttributes = serde_json::from_str(&ser).unwrap();

        assert_eq!(attributes, de);
    }
}
