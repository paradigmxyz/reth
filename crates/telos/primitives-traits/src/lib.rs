use alloy_primitives::private::proptest::collection::vec;
use alloy_primitives::U256;
use arbitrary::Arbitrary;
use serde::{Deserialize, Serialize};
use reth_codecs::Compact;

/// Telos block extension fields, included in Headers table as part of Header
#[derive(Debug, Arbitrary, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Compact)]
pub struct TelosBlockExtension {
    /// Gas prices for this block
    gas_prices: Vec<GasPrice>,
    /// Revision number for this block
    revision_numbers: Vec<Revision>,
}

impl TelosBlockExtension {
    /// Create a new Telos block extension for a child block, starting with the last price/revision from this one
    pub fn to_child(&self) -> Self {
        Self {
            gas_prices: vec![self.gas_prices.last().unwrap().clone()],
            revision_numbers: vec![self.revision_numbers.last().unwrap().clone()],
        }
    }
}

/// Telos gas price
#[derive(Debug, Arbitrary, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Compact)]
pub struct GasPrice {
    /// Transaction height
    pub height: u64,
    /// Value
    pub price: U256,
}

/// Telos revision number
#[derive(Debug, Arbitrary, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Compact)]
pub struct Revision {
    /// Transaction height
    pub height: u64,
    /// Revision
    pub revision: u64,
}