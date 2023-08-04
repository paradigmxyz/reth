use reth_primitives::{AccessList, Address, BlockId, Bytes, U256, U64, U8};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::BlockOverrides;

/// Bundle of transactions
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bundle {
    /// Transactions
    pub transactions: Vec<CallRequest>,
    /// Block overides
    pub block_override: Option<BlockOverrides>,
}

/// State context for callMany
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StateContext {
    /// Block Number
    pub block_number: Option<BlockId>,
    /// Inclusive number of tx to replay in block. -1 means replay all
    pub transaction_index: Option<TransactionIndex>,
}

/// Represents a transaction index where -1 means all transactions
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum TransactionIndex {
    /// -1 means all transactions
    #[default]
    All,
    /// Transaction index
    Index(usize),
}

impl TransactionIndex {
    /// Returns true if this is the all variant
    pub fn is_all(&self) -> bool {
        matches!(self, TransactionIndex::All)
    }

    /// Returns the index if this is the index variant
    pub fn index(&self) -> Option<usize> {
        match self {
            TransactionIndex::All => None,
            TransactionIndex::Index(idx) => Some(*idx),
        }
    }
}

impl Serialize for TransactionIndex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            TransactionIndex::All => serializer.serialize_i8(-1),
            TransactionIndex::Index(idx) => idx.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TransactionIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match isize::deserialize(deserializer)? {
            -1 => Ok(TransactionIndex::All),
            idx if idx < -1 => Err(serde::de::Error::custom(format!(
                "Invalid transaction index, expected -1 or positive integer, got {}",
                idx
            ))),
            idx => Ok(TransactionIndex::Index(idx as usize)),
        }
    }
}

/// Call request
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CallRequest {
    /// From
    pub from: Option<Address>,
    /// To
    pub to: Option<Address>,
    /// Gas Price
    pub gas_price: Option<U256>,
    /// EIP-1559 Max base fee the caller is willing to pay
    pub max_fee_per_gas: Option<U256>,
    /// EIP-1559 Priority fee the caller is paying to the block author
    pub max_priority_fee_per_gas: Option<U256>,
    /// Gas
    pub gas: Option<U256>,
    /// Value
    pub value: Option<U256>,
    /// Transaction input data
    #[serde(default, flatten)]
    pub input: CallInput,
    /// Nonce
    pub nonce: Option<U256>,
    /// chain id
    pub chain_id: Option<U64>,
    /// AccessList
    pub access_list: Option<AccessList>,
    /// EIP-2718 type
    #[serde(rename = "type")]
    pub transaction_type: Option<U8>,
}

impl CallRequest {
    /// Returns the configured fee cap, if any.
    ///
    /// The returns `gas_price` (legacy) if set or `max_fee_per_gas` (EIP1559)
    pub fn fee_cap(&self) -> Option<U256> {
        self.gas_price.or(self.max_fee_per_gas)
    }
}

/// Helper type that supports both `data` and `input` fields that map to transaction input data.
///
/// This is done for compatibility reasons where older implementations used `data` instead of the
/// newer, recommended `input` field.
///
/// If both fields are set, it is expected that they contain the same value, otherwise an error is
/// returned.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CallInput {
    /// Transaction data
    pub input: Option<Bytes>,
    /// Transaction data
    ///
    /// This is the same as `input` but is used for backwards compatibility: <https://github.com/ethereum/go-ethereum/issues/15628>
    pub data: Option<Bytes>,
}

impl CallInput {
    /// Consumes the type and returns the optional input data.
    ///
    /// Returns an error if both `data` and `input` fields are set and not equal.
    pub fn try_into_unique_input(self) -> Result<Option<Bytes>, CallInputError> {
        let Self { input, data } = self;
        match (input, data) {
            (Some(input), Some(data)) if input == data => Ok(Some(input)),
            (Some(_), Some(_)) => Err(CallInputError::default()),
            (Some(input), None) => Ok(Some(input)),
            (None, Some(data)) => Ok(Some(data)),
            (None, None) => Ok(None),
        }
    }

    /// Consumes the type and returns the optional input data.
    ///
    /// Returns an error if both `data` and `input` fields are set and not equal.
    pub fn unique_input(&self) -> Result<Option<&Bytes>, CallInputError> {
        let Self { input, data } = self;
        match (input, data) {
            (Some(input), Some(data)) if input == data => Ok(Some(input)),
            (Some(_), Some(_)) => Err(CallInputError::default()),
            (Some(input), None) => Ok(Some(input)),
            (None, Some(data)) => Ok(Some(data)),
            (None, None) => Ok(None),
        }
    }
}

impl From<Bytes> for CallInput {
    fn from(input: Bytes) -> Self {
        Self { input: Some(input), data: None }
    }
}

impl From<Option<Bytes>> for CallInput {
    fn from(input: Option<Bytes>) -> Self {
        Self { input, data: None }
    }
}

/// Error thrown when both `data` and `input` fields are set and not equal.
#[derive(Debug, Default, thiserror::Error)]
#[error("both \"data\" and \"input\" are set and not equal. Please use \"input\" to pass transaction call data")]
#[non_exhaustive]
pub struct CallInputError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_index() {
        let s = "-1";
        let idx = serde_json::from_str::<TransactionIndex>(s).unwrap();
        assert_eq!(idx, TransactionIndex::All);

        let s = "5";
        let idx = serde_json::from_str::<TransactionIndex>(s).unwrap();
        assert_eq!(idx, TransactionIndex::Index(5));

        let s = "-2";
        let res = serde_json::from_str::<TransactionIndex>(s);
        assert!(res.is_err());
    }

    #[test]
    fn serde_call_request() {
        let s = r#"{"accessList":[],"data":"0x0902f1ac","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let _req = serde_json::from_str::<CallRequest>(s).unwrap();
    }

    #[test]
    fn serde_unique_call_input() {
        let s = r#"{"accessList":[],"data":"0x0902f1ac", "input":"0x0902f1ac","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let req = serde_json::from_str::<CallRequest>(s).unwrap();
        assert!(req.input.try_into_unique_input().unwrap().is_some());

        let s = r#"{"accessList":[],"data":"0x0902f1ac","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let req = serde_json::from_str::<CallRequest>(s).unwrap();
        assert!(req.input.try_into_unique_input().unwrap().is_some());

        let s = r#"{"accessList":[],"input":"0x0902f1ac","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let req = serde_json::from_str::<CallRequest>(s).unwrap();
        assert!(req.input.try_into_unique_input().unwrap().is_some());

        let s = r#"{"accessList":[],"data":"0x0902f1ac", "input":"0x0902f1","to":"0xa478c2975ab1ea89e8196811f51a7b7ade33eb11","type":"0x02"}"#;
        let req = serde_json::from_str::<CallRequest>(s).unwrap();
        assert!(req.input.try_into_unique_input().is_err());
    }
}
