//! MEV bundle type bindings

#![allow(missing_docs)]
use crate::{BlockId, BlockNumberOrTag, Log};
use alloy_primitives::{Address, Bytes, TxHash, B256, U256, U64};
use serde::{
    ser::{SerializeSeq, Serializer},
    Deserialize, Deserializer, Serialize,
};

/// A bundle of transactions to send to the matchmaker.
///
/// Note: this is for `mev_sendBundle` and not `eth_sendBundle`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendBundleRequest {
    /// The version of the MEV-share API to use.
    #[serde(rename = "version")]
    pub protocol_version: ProtocolVersion,
    /// Data used by block builders to check if the bundle should be considered for inclusion.
    #[serde(rename = "inclusion")]
    pub inclusion: Inclusion,
    /// The transactions to include in the bundle.
    #[serde(rename = "body")]
    pub bundle_body: Vec<BundleItem>,
    /// Requirements for the bundle to be included in the block.
    #[serde(rename = "validity", skip_serializing_if = "Option::is_none")]
    pub validity: Option<Validity>,
    /// Preferences on what data should be shared about the bundle and its transactions
    #[serde(rename = "privacy", skip_serializing_if = "Option::is_none")]
    pub privacy: Option<Privacy>,
}

/// Data used by block builders to check if the bundle should be considered for inclusion.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Inclusion {
    /// The first block the bundle is valid for.
    pub block: U64,
    /// The last block the bundle is valid for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_block: Option<U64>,
}

impl Inclusion {
    /// Creates a new inclusion with the given min block..
    pub fn at_block(block: u64) -> Self {
        Self { block: U64::from(block), max_block: None }
    }

    /// Returns the block number of the first block the bundle is valid for.
    #[inline]
    pub fn block_number(&self) -> u64 {
        self.block.to()
    }

    /// Returns the block number of the last block the bundle is valid for.
    #[inline]
    pub fn max_block_number(&self) -> Option<u64> {
        self.max_block.as_ref().map(|b| b.to())
    }
}

/// A bundle tx, which can either be a transaction hash, or a full tx.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
#[serde(rename_all = "camelCase")]
pub enum BundleItem {
    /// The hash of either a transaction or bundle we are trying to backrun.
    Hash {
        /// Tx hash.
        hash: TxHash,
    },
    /// A new signed transaction.
    #[serde(rename_all = "camelCase")]
    Tx {
        /// Bytes of the signed transaction.
        tx: Bytes,
        /// If true, the transaction can revert without the bundle being considered invalid.
        can_revert: bool,
    },
}

/// Requirements for the bundle to be included in the block.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Validity {
    /// Specifies the minimum percent of a given bundle's earnings to redistribute
    /// for it to be included in a builder's block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund: Option<Vec<Refund>>,
    /// Specifies what addresses should receive what percent of the overall refund for this bundle,
    /// if it is enveloped by another bundle (eg. a searcher backrun).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_config: Option<Vec<RefundConfig>>,
}

/// Specifies the minimum percent of a given bundle's earnings to redistribute
/// for it to be included in a builder's block.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Refund {
    /// The index of the transaction in the bundle.
    pub body_idx: u64,
    /// The minimum percent of the bundle's earnings to redistribute.
    pub percent: u64,
}

/// Specifies what addresses should receive what percent of the overall refund for this bundle,
/// if it is enveloped by another bundle (eg. a searcher backrun).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RefundConfig {
    /// The address to refund.
    pub address: Address,
    /// The minimum percent of the bundle's earnings to redistribute.
    pub percent: u64,
}

/// Preferences on what data should be shared about the bundle and its transactions
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Privacy {
    /// Hints on what data should be shared about the bundle and its transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hints: Option<PrivacyHint>,
    /// The addresses of the builders that should be allowed to see the bundle/transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builders: Option<Vec<Address>>,
}

/// Hints on what data should be shared about the bundle and its transactions
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrivacyHint {
    /// The calldata of the bundle's transactions should be shared.
    pub calldata: bool,
    /// The address of the bundle's transactions should be shared.
    pub contract_address: bool,
    /// The logs of the bundle's transactions should be shared.
    pub logs: bool,
    /// The function selector of the bundle's transactions should be shared.
    pub function_selector: bool,
    /// The hash of the bundle's transactions should be shared.
    pub hash: bool,
    /// The hash of the bundle should be shared.
    pub tx_hash: bool,
}

impl PrivacyHint {
    pub fn with_calldata(mut self) -> Self {
        self.calldata = true;
        self
    }

    pub fn with_contract_address(mut self) -> Self {
        self.contract_address = true;
        self
    }

    pub fn with_logs(mut self) -> Self {
        self.logs = true;
        self
    }

    pub fn with_function_selector(mut self) -> Self {
        self.function_selector = true;
        self
    }

    pub fn with_hash(mut self) -> Self {
        self.hash = true;
        self
    }

    pub fn with_tx_hash(mut self) -> Self {
        self.tx_hash = true;
        self
    }

    pub fn has_calldata(&self) -> bool {
        self.calldata
    }

    pub fn has_contract_address(&self) -> bool {
        self.contract_address
    }

    pub fn has_logs(&self) -> bool {
        self.logs
    }

    pub fn has_function_selector(&self) -> bool {
        self.function_selector
    }

    pub fn has_hash(&self) -> bool {
        self.hash
    }

    pub fn has_tx_hash(&self) -> bool {
        self.tx_hash
    }

    fn num_hints(&self) -> usize {
        let mut num_hints = 0;
        if self.calldata {
            num_hints += 1;
        }
        if self.contract_address {
            num_hints += 1;
        }
        if self.logs {
            num_hints += 1;
        }
        if self.function_selector {
            num_hints += 1;
        }
        if self.hash {
            num_hints += 1;
        }
        if self.tx_hash {
            num_hints += 1;
        }
        num_hints
    }
}

impl Serialize for PrivacyHint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.num_hints()))?;
        if self.calldata {
            seq.serialize_element("calldata")?;
        }
        if self.contract_address {
            seq.serialize_element("contract_address")?;
        }
        if self.logs {
            seq.serialize_element("logs")?;
        }
        if self.function_selector {
            seq.serialize_element("function_selector")?;
        }
        if self.hash {
            seq.serialize_element("hash")?;
        }
        if self.tx_hash {
            seq.serialize_element("tx_hash")?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for PrivacyHint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hints = Vec::<String>::deserialize(deserializer)?;
        let mut privacy_hint = PrivacyHint::default();
        for hint in hints {
            match hint.as_str() {
                "calldata" => privacy_hint.calldata = true,
                "contract_address" => privacy_hint.contract_address = true,
                "logs" => privacy_hint.logs = true,
                "function_selector" => privacy_hint.function_selector = true,
                "hash" => privacy_hint.hash = true,
                "tx_hash" => privacy_hint.tx_hash = true,
                _ => return Err(serde::de::Error::custom("invalid privacy hint")),
            }
        }
        Ok(privacy_hint)
    }
}

/// Response from the matchmaker after sending a bundle.
#[derive(Deserialize, Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SendBundleResponse {
    /// Hash of the bundle bodies.
    pub bundle_hash: B256,
}

/// The version of the MEV-share API to use.
#[derive(Deserialize, Debug, Serialize, Clone, Default, PartialEq, Eq)]
pub enum ProtocolVersion {
    #[default]
    #[serde(rename = "beta-1")]
    /// The beta-1 version of the API.
    Beta1,
    /// The 0.1 version of the API.
    #[serde(rename = "v0.1")]
    V0_1,
}

/// Optional fields to override simulation state.
#[derive(Deserialize, Debug, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SimBundleOverrides {
    /// Block used for simulation state. Defaults to latest block.
    /// Block header data will be derived from parent block by default.
    /// Specify other params to override the default values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_block: Option<BlockId>,
    /// Block number used for simulation, defaults to parentBlock.number + 1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_number: Option<U64>,
    /// Coinbase used for simulation, defaults to parentBlock.coinbase
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coinbase: Option<Address>,
    /// Timestamp used for simulation, defaults to parentBlock.timestamp + 12
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<U64>,
    /// Gas limit used for simulation, defaults to parentBlock.gasLimit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gas_limit: Option<U64>,
    /// Base fee used for simulation, defaults to parentBlock.baseFeePerGas
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_fee: Option<U64>,
    /// Timeout in seconds, defaults to 5
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<U64>,
}

/// Response from the matchmaker after sending a simulation request.
#[derive(Deserialize, Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SimBundleResponse {
    /// Whether the simulation was successful.
    pub success: bool,
    /// Error message if the simulation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The block number of the simulated block.
    pub state_block: U64,
    /// The gas price of the simulated block.
    pub mev_gas_price: U64,
    /// The profit of the simulated block.
    pub profit: U64,
    /// The refundable value of the simulated block.
    pub refundable_value: U64,
    /// The gas used by the simulated block.
    pub gas_used: U64,
    /// Logs returned by mev_simBundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<Vec<SimBundleLogs>>,
}

/// Logs returned by mev_simBundle.
#[derive(Deserialize, Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SimBundleLogs {
    /// Logs for transactions in bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_logs: Option<Vec<Log>>,
    /// Logs for bundles in bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_logs: Option<Vec<SimBundleLogs>>,
}

impl SendBundleRequest {
    /// Create a new bundle request.
    pub fn new(
        block_num: U64,
        max_block: Option<U64>,
        protocol_version: ProtocolVersion,
        bundle_body: Vec<BundleItem>,
    ) -> Self {
        Self {
            protocol_version,
            inclusion: Inclusion { block: block_num, max_block },
            bundle_body,
            validity: None,
            privacy: None,
        }
    }
}

/// Request for `eth_cancelBundle`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CancelBundleRequest {
    /// Bundle hash of the bundle to be canceled
    pub bundle_hash: String,
}

/// Request for `eth_sendPrivateTransaction`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PrivateTransactionRequest {
    /// raw signed transaction
    pub tx: Bytes,
    /// Hex-encoded number string, optional. Highest block number in which the transaction should
    /// be included.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_block_number: Option<U64>,
    #[serde(default, skip_serializing_if = "PrivateTransactionPreferences::is_empty")]
    pub preferences: PrivateTransactionPreferences,
}

/// Additional preferences for `eth_sendPrivateTransaction`
#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq, Eq)]
pub struct PrivateTransactionPreferences {
    /// Requirements for the bundle to be included in the block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validity: Option<Validity>,
    /// Preferences on what data should be shared about the bundle and its transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub privacy: Option<Privacy>,
}

impl PrivateTransactionPreferences {
    /// Returns true if the preferences are empty.
    pub fn is_empty(&self) -> bool {
        self.validity.is_none() && self.privacy.is_none()
    }
}

/// Request for `eth_cancelPrivateTransaction`
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CancelPrivateTransactionRequest {
    /// Transaction hash of the transaction to be canceled
    pub tx_hash: B256,
}

// TODO(@optimiz-r): Revisit after <https://github.com/flashbots/flashbots-docs/issues/424> is closed.
/// Response for `flashbots_getBundleStatsV2` represents stats for a single bundle
///
/// Note: this is V2: <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#flashbots_getbundlestatsv2>
///
/// Timestamp format: "2022-10-06T21:36:06.322Z"
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum BundleStats {
    /// The relayer has not yet seen the bundle.
    #[default]
    Unknown,
    /// The relayer has seen the bundle, but has not simulated it yet.
    Seen(StatsSeen),
    /// The relayer has seen the bundle and has simulated it.
    Simulated(StatsSimulated),
}

impl Serialize for BundleStats {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            BundleStats::Unknown => serde_json::json!({"isSimulated": false}).serialize(serializer),
            BundleStats::Seen(stats) => stats.serialize(serializer),
            BundleStats::Simulated(stats) => stats.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for BundleStats {
    fn deserialize<D>(deserializer: D) -> Result<BundleStats, D::Error>
    where
        D: Deserializer<'de>,
    {
        let map = serde_json::Map::deserialize(deserializer)?;

        if map.get("receivedAt").is_none() {
            Ok(BundleStats::Unknown)
        } else if map["isSimulated"] == false {
            StatsSeen::deserialize(serde_json::Value::Object(map))
                .map(BundleStats::Seen)
                .map_err(serde::de::Error::custom)
        } else {
            StatsSimulated::deserialize(serde_json::Value::Object(map))
                .map(BundleStats::Simulated)
                .map_err(serde::de::Error::custom)
        }
    }
}

/// Response for `flashbots_getBundleStatsV2` represents stats for a single bundle
///
/// Note: this is V2: <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#flashbots_getbundlestatsv2>
///
/// Timestamp format: "2022-10-06T21:36:06.322Z
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSeen {
    /// boolean representing if this searcher has a high enough reputation to be in the high
    /// priority queue
    pub is_high_priority: bool,
    /// representing whether the bundle gets simulated. All other fields will be omitted except
    /// simulated field if API didn't receive bundle
    pub is_simulated: bool,
    /// time at which the bundle API received the bundle
    pub received_at: String,
}

/// Response for `flashbots_getBundleStatsV2` represents stats for a single bundle
///
/// Note: this is V2: <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#flashbots_getbundlestatsv2>
///
/// Timestamp format: "2022-10-06T21:36:06.322Z
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSimulated {
    /// boolean representing if this searcher has a high enough reputation to be in the high
    /// priority queue
    pub is_high_priority: bool,
    /// representing whether the bundle gets simulated. All other fields will be omitted except
    /// simulated field if API didn't receive bundle
    pub is_simulated: bool,
    /// time at which the bundle gets simulated
    pub simulated_at: String,
    /// time at which the bundle API received the bundle
    pub received_at: String,
    /// indicates time at which each builder selected the bundle to be included in the target
    /// block
    #[serde(default = "Vec::new")]
    pub considered_by_builders_at: Vec<ConsideredByBuildersAt>,
    /// indicates time at which each builder sealed a block containing the bundle
    #[serde(default = "Vec::new")]
    pub sealed_by_builders_at: Vec<SealedByBuildersAt>,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsideredByBuildersAt {
    pub pubkey: String,
    pub timestamp: String,
}

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedByBuildersAt {
    pub pubkey: String,
    pub timestamp: String,
}

/// Response for `flashbots_getUserStatsV2` represents stats for a searcher.
///
/// Note: this is V2: <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#flashbots_getuserstatsv2>
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserStats {
    /// Represents whether this searcher has a high enough reputation to be in the high priority
    /// queue.
    pub is_high_priority: bool,
    /// The total amount paid to validators over all time.
    #[serde(with = "u256_numeric_string")]
    pub all_time_validator_payments: U256,
    /// The total amount of gas simulated across all bundles submitted to Flashbots.
    /// This is the actual gas used in simulations, not gas limit.
    #[serde(with = "u256_numeric_string")]
    pub all_time_gas_simulated: U256,
    /// The total amount paid to validators the last 7 days.
    #[serde(with = "u256_numeric_string")]
    pub last_7d_validator_payments: U256,
    /// The total amount of gas simulated across all bundles submitted to Flashbots in the last 7
    /// days. This is the actual gas used in simulations, not gas limit.
    #[serde(with = "u256_numeric_string")]
    pub last_7d_gas_simulated: U256,
    /// The total amount paid to validators the last day.
    #[serde(with = "u256_numeric_string")]
    pub last_1d_validator_payments: U256,
    /// The total amount of gas simulated across all bundles submitted to Flashbots in the last
    /// day. This is the actual gas used in simulations, not gas limit.
    #[serde(with = "u256_numeric_string")]
    pub last_1d_gas_simulated: U256,
}

/// Bundle of transactions for `eth_sendBundle`
///
/// Note: this is for `eth_sendBundle` and not `mev_sendBundle`
///
/// <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#eth_sendbundle>
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthSendBundle {
    /// A list of hex-encoded signed transactions
    pub txs: Vec<Bytes>,
    /// hex-encoded block number for which this bundle is valid
    pub block_number: U64,
    /// unix timestamp when this bundle becomes active
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_timestamp: Option<u64>,
    /// unix timestamp how long this bundle stays valid
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_timestamp: Option<u64>,
    /// list of hashes of possibly reverting txs
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reverting_tx_hashes: Vec<B256>,
    /// UUID that can be used to cancel/replace this bundle
    #[serde(rename = "replacementUuid", skip_serializing_if = "Option::is_none")]
    pub replacement_uuid: Option<String>,
}

/// Response from the matchmaker after sending a bundle.
#[derive(Deserialize, Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EthBundleHash {
    /// Hash of the bundle bodies.
    pub bundle_hash: B256,
}

/// Bundle of transactions for `eth_callBundle`
///
/// <https://docs.flashbots.net/flashbots-auction/searchers/advanced/rpc-endpoint#eth_callBundle>
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCallBundle {
    /// A list of hex-encoded signed transactions
    pub txs: Vec<Bytes>,
    /// hex encoded block number for which this bundle is valid on
    pub block_number: U64,
    /// Either a hex encoded number or a block tag for which state to base this simulation on
    pub state_block_number: BlockNumberOrTag,
    /// the timestamp to use for this bundle simulation, in seconds since the unix epoch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

/// Response for `eth_callBundle`
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EthCallBundleResponse {
    pub bundle_hash: B256,
    #[serde(with = "u256_numeric_string")]
    pub bundle_gas_price: U256,
    #[serde(with = "u256_numeric_string")]
    pub coinbase_diff: U256,
    #[serde(with = "u256_numeric_string")]
    pub eth_sent_to_coinbase: U256,
    #[serde(with = "u256_numeric_string")]
    pub gas_fees: U256,
    pub results: Vec<EthCallBundleTransactionResult>,
    pub state_block_number: u64,
    pub total_gas_used: u64,
}

/// Result of a single transaction in a bundle for `eth_callBundle`
#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthCallBundleTransactionResult {
    #[serde(with = "u256_numeric_string")]
    pub coinbase_diff: U256,
    #[serde(with = "u256_numeric_string")]
    pub eth_sent_to_coinbase: U256,
    pub from_address: Address,
    #[serde(with = "u256_numeric_string")]
    pub gas_fees: U256,
    #[serde(with = "u256_numeric_string")]
    pub gas_price: U256,
    pub gas_used: u64,
    pub to_address: Option<Address>,
    pub tx_hash: B256,
    /// Contains the return data if the transaction succeeded
    ///
    /// Note: this is mutually exclusive with `revert`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Bytes>,
    /// Contains the return data if the transaction reverted
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revert: Option<Bytes>,
}

mod u256_numeric_string {
    use alloy_primitives::U256;
    use serde::{de, Deserialize, Serializer};
    use std::str::FromStr;

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<U256, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let val = serde_json::Value::deserialize(deserializer)?;
        match val {
            serde_json::Value::String(s) => {
                if let Ok(val) = s.parse::<u128>() {
                    return Ok(U256::from(val))
                }
                U256::from_str(&s).map_err(de::Error::custom)
            }
            serde_json::Value::Number(num) => {
                num.as_u64().map(U256::from).ok_or_else(|| de::Error::custom("invalid u256"))
            }
            _ => Err(de::Error::custom("invalid u256")),
        }
    }

    pub(crate) fn serialize<S>(val: &U256, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let val: u128 = (*val).try_into().map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&val.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use std::str::FromStr;

    #[test]
    fn can_deserialize_simple() {
        let str = r#"
        [{
            "version": "v0.1",
            "inclusion": {
                "block": "0x1"
            },
            "body": [{
                "tx": "0x02f86b0180843b9aca00852ecc889a0082520894c87037874aed04e51c29f582394217a0a2b89d808080c080a0a463985c616dd8ee17d7ef9112af4e6e06a27b071525b42182fe7b0b5c8b4925a00af5ca177ffef2ff28449292505d41be578bebb77110dfc09361d2fb56998260",
                "canRevert": false
            }]
        }]
        "#;
        let res: Result<Vec<SendBundleRequest>, _> = serde_json::from_str(str);
        assert!(res.is_ok());
    }

    #[test]
    fn can_deserialize_complex() {
        let str = r#"
        [{
            "version": "v0.1",
            "inclusion": {
                "block": "0x1"
            },
            "body": [{
                "tx": "0x02f86b0180843b9aca00852ecc889a0082520894c87037874aed04e51c29f582394217a0a2b89d808080c080a0a463985c616dd8ee17d7ef9112af4e6e06a27b071525b42182fe7b0b5c8b4925a00af5ca177ffef2ff28449292505d41be578bebb77110dfc09361d2fb56998260",
                "canRevert": false
            }],
            "privacy": {
                "hints": [
                  "calldata"
                ]
              },
              "validity": {
                "refundConfig": [
                  {
                    "address": "0x8EC1237b1E80A6adf191F40D4b7D095E21cdb18f",
                    "percent": 100
                  }
                ]
              }
        }]
        "#;
        let res: Result<Vec<SendBundleRequest>, _> = serde_json::from_str(str);
        assert!(res.is_ok());
    }

    #[test]
    fn can_serialize_complex() {
        let str = r#"
        [{
            "version": "v0.1",
            "inclusion": {
                "block": "0x1"
            },
            "body": [{
                "tx": "0x02f86b0180843b9aca00852ecc889a0082520894c87037874aed04e51c29f582394217a0a2b89d808080c080a0a463985c616dd8ee17d7ef9112af4e6e06a27b071525b42182fe7b0b5c8b4925a00af5ca177ffef2ff28449292505d41be578bebb77110dfc09361d2fb56998260",
                "canRevert": false
            }],
            "privacy": {
                "hints": [
                  "calldata"
                ]
              },
              "validity": {
                "refundConfig": [
                  {
                    "address": "0x8EC1237b1E80A6adf191F40D4b7D095E21cdb18f",
                    "percent": 100
                  }
                ]
              }
        }]
        "#;
        let bundle_body = vec![BundleItem::Tx {
            tx: Bytes::from_str("0x02f86b0180843b9aca00852ecc889a0082520894c87037874aed04e51c29f582394217a0a2b89d808080c080a0a463985c616dd8ee17d7ef9112af4e6e06a27b071525b42182fe7b0b5c8b4925a00af5ca177ffef2ff28449292505d41be578bebb77110dfc09361d2fb56998260").unwrap(),
            can_revert: false,
        }];

        let validity = Some(Validity {
            refund_config: Some(vec![RefundConfig {
                address: "0x8EC1237b1E80A6adf191F40D4b7D095E21cdb18f".parse().unwrap(),
                percent: 100,
            }]),
            ..Default::default()
        });

        let privacy = Some(Privacy {
            hints: Some(PrivacyHint { calldata: true, ..Default::default() }),
            ..Default::default()
        });

        let bundle = SendBundleRequest {
            protocol_version: ProtocolVersion::V0_1,
            inclusion: Inclusion { block: U64::from(1), max_block: None },
            bundle_body,
            validity,
            privacy,
        };
        let expected = serde_json::from_str::<Vec<SendBundleRequest>>(str).unwrap();
        assert_eq!(bundle, expected[0]);
    }

    #[test]
    fn can_serialize_privacy_hint() {
        let hint = PrivacyHint {
            calldata: true,
            contract_address: true,
            logs: true,
            function_selector: true,
            hash: true,
            tx_hash: true,
        };
        let expected =
            r#"["calldata","contract_address","logs","function_selector","hash","tx_hash"]"#;
        let actual = serde_json::to_string(&hint).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn can_deserialize_privacy_hint() {
        let hint = PrivacyHint {
            calldata: true,
            contract_address: false,
            logs: true,
            function_selector: false,
            hash: true,
            tx_hash: false,
        };
        let expected = r#"["calldata","logs","hash"]"#;
        let actual: PrivacyHint = serde_json::from_str(expected).unwrap();
        assert_eq!(actual, hint);
    }

    #[test]
    fn can_dererialize_sim_response() {
        let expected = r#"
        {
            "success": true,
            "stateBlock": "0x8b8da8",
            "mevGasPrice": "0x74c7906005",
            "profit": "0x4bc800904fc000",
            "refundableValue": "0x4bc800904fc000",
            "gasUsed": "0xa620",
            "logs": [{},{}]
          }
        "#;
        let actual: SimBundleResponse = serde_json::from_str(expected).unwrap();
        assert!(actual.success);
    }

    #[test]
    fn can_deserialize_eth_call_resp() {
        let s = r#"{
    "bundleGasPrice": "476190476193",
    "bundleHash": "0x73b1e258c7a42fd0230b2fd05529c5d4b6fcb66c227783f8bece8aeacdd1db2e",
    "coinbaseDiff": "20000000000126000",
    "ethSentToCoinbase": "20000000000000000",
    "gasFees": "126000",
    "results": [
      {
        "coinbaseDiff": "10000000000063000",
        "ethSentToCoinbase": "10000000000000000",
        "fromAddress": "0x02A727155aeF8609c9f7F2179b2a1f560B39F5A0",
        "gasFees": "63000",
        "gasPrice": "476190476193",
        "gasUsed": 21000,
        "toAddress": "0x73625f59CAdc5009Cb458B751b3E7b6b48C06f2C",
        "txHash": "0x669b4704a7d993a946cdd6e2f95233f308ce0c4649d2e04944e8299efcaa098a",
        "value": "0x"
      },
      {
        "coinbaseDiff": "10000000000063000",
        "ethSentToCoinbase": "10000000000000000",
        "fromAddress": "0x02A727155aeF8609c9f7F2179b2a1f560B39F5A0",
        "gasFees": "63000",
        "gasPrice": "476190476193",
        "gasUsed": 21000,
        "toAddress": "0x73625f59CAdc5009Cb458B751b3E7b6b48C06f2C",
        "txHash": "0xa839ee83465657cac01adc1d50d96c1b586ed498120a84a64749c0034b4f19fa",
        "value": "0x"
      }
    ],
    "stateBlockNumber": 5221585,
    "totalGasUsed": 42000
  }"#;

        let _call = serde_json::from_str::<EthCallBundleResponse>(s).unwrap();
    }

    #[test]
    fn can_serialize_deserialize_bundle_stats() {
        let fixtures = [
            (
                r#"{
                    "isSimulated": false
                }"#,
                BundleStats::Unknown,
            ),
            (
                r#"{
                    "isHighPriority": false,
                    "isSimulated": false,
                    "receivedAt": "476190476193"
                }"#,
                BundleStats::Seen(StatsSeen {
                    is_high_priority: false,
                    is_simulated: false,
                    received_at: "476190476193".to_string(),
                }),
            ),
            (
                r#"{
                    "isHighPriority": true,
                    "isSimulated": true,
                    "simulatedAt": "111",
                    "receivedAt": "222",
                    "consideredByBuildersAt":[],
                    "sealedByBuildersAt": [
                        {
                            "pubkey": "333",
                            "timestamp": "444"
                        },
                        {
                            "pubkey": "555",
                            "timestamp": "666"
                        }
                    ]
                }"#,
                BundleStats::Simulated(StatsSimulated {
                    is_high_priority: true,
                    is_simulated: true,
                    simulated_at: String::from("111"),
                    received_at: String::from("222"),
                    considered_by_builders_at: vec![],
                    sealed_by_builders_at: vec![
                        SealedByBuildersAt {
                            pubkey: String::from("333"),
                            timestamp: String::from("444"),
                        },
                        SealedByBuildersAt {
                            pubkey: String::from("555"),
                            timestamp: String::from("666"),
                        },
                    ],
                }),
            ),
        ];

        let strip_whitespaces =
            |input: &str| input.chars().filter(|&c| !c.is_whitespace()).collect::<String>();

        for (serialized, deserialized) in fixtures {
            // Check de-serialization
            let deserialized_expected = serde_json::from_str::<BundleStats>(serialized).unwrap();
            assert_eq!(deserialized, deserialized_expected);

            // Check serialization
            let serialized_expected = &serde_json::to_string(&deserialized).unwrap();
            assert_eq!(strip_whitespaces(serialized), strip_whitespaces(serialized_expected));
        }
    }
}
