//! Payload support for the beacon API.
//!
//! Internal helper module to deserialize/serialize the payload attributes for the beacon API, which
//! uses snake case and quoted decimals.
//!
//! This is necessary because we don't want to allow a mixture of both formats, hence `serde`
//! aliases are not an option.
//!
//! See also <https://github.com/ethereum/consensus-specs/blob/master/specs/deneb/beacon-chain.md#executionpayload>

#![allow(missing_docs)]

use crate::{
    beacon::{withdrawals::BeaconWithdrawal, BlsPublicKey},
    engine::{ExecutionPayload, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3},
    Withdrawal,
};
use alloy_primitives::{Address, Bloom, Bytes, B256, U256};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, DisplayFromStr, SerializeAs};
use std::borrow::Cow;

/// Response object of GET `/eth/v1/builder/header/{slot}/{parent_hash}/{pubkey}`
///
/// See also <https://ethereum.github.io/builder-specs/#/Builder/getHeader>
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GetExecutionPayloadHeaderResponse {
    pub version: String,
    pub data: ExecutionPayloadHeaderData,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPayloadHeaderData {
    pub message: ExecutionPayloadHeaderMessage,
    pub signature: String,
}

#[serde_as]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPayloadHeaderMessage {
    pub header: ExecutionPayloadHeader,
    #[serde_as(as = "DisplayFromStr")]
    pub value: U256,
    pub pubkey: BlsPublicKey,
}

/// The header of the execution payload.
#[serde_as]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionPayloadHeader {
    pub parent_hash: B256,
    pub fee_recipient: Address,
    pub state_root: B256,
    pub receipts_root: B256,
    pub logs_bloom: Bloom,
    pub prev_randao: B256,
    #[serde_as(as = "DisplayFromStr")]
    pub block_number: String,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_limit: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub gas_used: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub timestamp: u64,
    pub extra_data: Bytes,
    #[serde_as(as = "DisplayFromStr")]
    pub base_fee_per_gas: U256,
    pub block_hash: B256,
    pub transactions_root: B256,
}

#[serde_as]
#[derive(Serialize, Deserialize)]
struct BeaconPayloadAttributes {
    #[serde_as(as = "DisplayFromStr")]
    timestamp: u64,
    prev_randao: B256,
    suggested_fee_recipient: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<Vec<BeaconWithdrawal>>")]
    withdrawals: Option<Vec<Withdrawal>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_beacon_block_root: Option<B256>,
}

/// Optimism Payload Attributes
#[serde_as]
#[derive(Serialize, Deserialize)]
struct BeaconOptimismPayloadAttributes {
    #[serde(flatten)]
    payload_attributes: BeaconPayloadAttributes,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transactions: Option<Vec<Bytes>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_tx_pool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<DisplayFromStr>")]
    gas_limit: Option<u64>,
}

/// A helper module for serializing and deserializing optimism payload attributes for the beacon
/// API.
///
/// See docs for [beacon_api_payload_attributes].
pub mod beacon_api_payload_attributes_optimism {
    use super::*;
    use crate::engine::{OptimismPayloadAttributes, PayloadAttributes};

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &OptimismPayloadAttributes,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let beacon_api_payload_attributes = BeaconPayloadAttributes {
            timestamp: payload_attributes.payload_attributes.timestamp,
            prev_randao: payload_attributes.payload_attributes.prev_randao,
            suggested_fee_recipient: payload_attributes.payload_attributes.suggested_fee_recipient,
            withdrawals: payload_attributes.payload_attributes.withdrawals.clone(),
            parent_beacon_block_root: payload_attributes
                .payload_attributes
                .parent_beacon_block_root,
        };

        let op_beacon_api_payload_attributes = BeaconOptimismPayloadAttributes {
            payload_attributes: beacon_api_payload_attributes,
            transactions: payload_attributes.transactions.clone(),
            no_tx_pool: payload_attributes.no_tx_pool,
            gas_limit: payload_attributes.gas_limit,
        };

        op_beacon_api_payload_attributes.serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<OptimismPayloadAttributes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let beacon_api_payload_attributes =
            BeaconOptimismPayloadAttributes::deserialize(deserializer)?;
        Ok(OptimismPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: beacon_api_payload_attributes.payload_attributes.timestamp,
                prev_randao: beacon_api_payload_attributes.payload_attributes.prev_randao,
                suggested_fee_recipient: beacon_api_payload_attributes
                    .payload_attributes
                    .suggested_fee_recipient,
                withdrawals: beacon_api_payload_attributes.payload_attributes.withdrawals,
                parent_beacon_block_root: beacon_api_payload_attributes
                    .payload_attributes
                    .parent_beacon_block_root,
            },
            transactions: beacon_api_payload_attributes.transactions,
            no_tx_pool: beacon_api_payload_attributes.no_tx_pool,
            gas_limit: beacon_api_payload_attributes.gas_limit,
        })
    }
}

/// A helper module for serializing and deserializing the payload attributes for the beacon API.
///
/// The beacon API encoded object has equivalent fields to the
/// [PayloadAttributes](crate::engine::PayloadAttributes) with two differences:
/// 1) `snake_case` identifiers must be used rather than `camelCase`;
/// 2) integers must be encoded as quoted decimals rather than big-endian hex.
pub mod beacon_api_payload_attributes {
    use super::*;
    use crate::engine::PayloadAttributes;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &PayloadAttributes,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let beacon_api_payload_attributes = BeaconPayloadAttributes {
            timestamp: payload_attributes.timestamp,
            prev_randao: payload_attributes.prev_randao,
            suggested_fee_recipient: payload_attributes.suggested_fee_recipient,
            withdrawals: payload_attributes.withdrawals.clone(),
            parent_beacon_block_root: payload_attributes.parent_beacon_block_root,
        };
        beacon_api_payload_attributes.serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<PayloadAttributes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let beacon_api_payload_attributes = BeaconPayloadAttributes::deserialize(deserializer)?;
        Ok(PayloadAttributes {
            timestamp: beacon_api_payload_attributes.timestamp,
            prev_randao: beacon_api_payload_attributes.prev_randao,
            suggested_fee_recipient: beacon_api_payload_attributes.suggested_fee_recipient,
            withdrawals: beacon_api_payload_attributes.withdrawals,
            parent_beacon_block_root: beacon_api_payload_attributes.parent_beacon_block_root,
        })
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
struct BeaconExecutionPayloadV1<'a> {
    parent_hash: Cow<'a, B256>,
    fee_recipient: Cow<'a, Address>,
    state_root: Cow<'a, B256>,
    receipts_root: Cow<'a, B256>,
    logs_bloom: Cow<'a, Bloom>,
    prev_randao: Cow<'a, B256>,
    #[serde_as(as = "DisplayFromStr")]
    block_number: u64,
    #[serde_as(as = "DisplayFromStr")]
    gas_limit: u64,
    #[serde_as(as = "DisplayFromStr")]
    gas_used: u64,
    #[serde_as(as = "DisplayFromStr")]
    timestamp: u64,
    extra_data: Cow<'a, Bytes>,
    #[serde_as(as = "DisplayFromStr")]
    base_fee_per_gas: U256,
    block_hash: Cow<'a, B256>,
    transactions: Cow<'a, Vec<Bytes>>,
}

impl<'a> From<BeaconExecutionPayloadV1<'a>> for ExecutionPayloadV1 {
    fn from(payload: BeaconExecutionPayloadV1<'a>) -> Self {
        let BeaconExecutionPayloadV1 {
            parent_hash,
            fee_recipient,
            state_root,
            receipts_root,
            logs_bloom,
            prev_randao,
            block_number,
            gas_limit,
            gas_used,
            timestamp,
            extra_data,
            base_fee_per_gas,
            block_hash,
            transactions,
        } = payload;
        ExecutionPayloadV1 {
            parent_hash: parent_hash.into_owned(),
            fee_recipient: fee_recipient.into_owned(),
            state_root: state_root.into_owned(),
            receipts_root: receipts_root.into_owned(),
            logs_bloom: logs_bloom.into_owned(),
            prev_randao: prev_randao.into_owned(),
            block_number,
            gas_limit,
            gas_used,
            timestamp,
            extra_data: extra_data.into_owned(),
            base_fee_per_gas,
            block_hash: block_hash.into_owned(),
            transactions: transactions.into_owned(),
        }
    }
}

impl<'a> From<&'a ExecutionPayloadV1> for BeaconExecutionPayloadV1<'a> {
    fn from(value: &'a ExecutionPayloadV1) -> Self {
        let ExecutionPayloadV1 {
            parent_hash,
            fee_recipient,
            state_root,
            receipts_root,
            logs_bloom,
            prev_randao,
            block_number,
            gas_limit,
            gas_used,
            timestamp,
            extra_data,
            base_fee_per_gas,
            block_hash,
            transactions,
        } = value;

        BeaconExecutionPayloadV1 {
            parent_hash: Cow::Borrowed(parent_hash),
            fee_recipient: Cow::Borrowed(fee_recipient),
            state_root: Cow::Borrowed(state_root),
            receipts_root: Cow::Borrowed(receipts_root),
            logs_bloom: Cow::Borrowed(logs_bloom),
            prev_randao: Cow::Borrowed(prev_randao),
            block_number: *block_number,
            gas_limit: *gas_limit,
            gas_used: *gas_used,
            timestamp: *timestamp,
            extra_data: Cow::Borrowed(extra_data),
            base_fee_per_gas: *base_fee_per_gas,
            block_hash: Cow::Borrowed(block_hash),
            transactions: Cow::Borrowed(transactions),
        }
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_payload_v1 {
    use super::*;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &ExecutionPayloadV1,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BeaconExecutionPayloadV1::from(payload_attributes).serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<ExecutionPayloadV1, D::Error>
    where
        D: Deserializer<'de>,
    {
        BeaconExecutionPayloadV1::deserialize(deserializer).map(Into::into)
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
struct BeaconExecutionPayloadV2<'a> {
    /// Inner V1 payload
    #[serde(flatten)]
    payload_inner: BeaconExecutionPayloadV1<'a>,
    /// Array of [`Withdrawal`] enabled with V2
    /// See <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/shanghai.md#executionpayloadv2>
    #[serde_as(as = "Vec<BeaconWithdrawal>")]
    withdrawals: Vec<Withdrawal>,
}

impl<'a> From<BeaconExecutionPayloadV2<'a>> for ExecutionPayloadV2 {
    fn from(payload: BeaconExecutionPayloadV2<'a>) -> Self {
        let BeaconExecutionPayloadV2 { payload_inner, withdrawals } = payload;
        ExecutionPayloadV2 { payload_inner: payload_inner.into(), withdrawals }
    }
}

impl<'a> From<&'a ExecutionPayloadV2> for BeaconExecutionPayloadV2<'a> {
    fn from(value: &'a ExecutionPayloadV2) -> Self {
        let ExecutionPayloadV2 { payload_inner, withdrawals } = value;
        BeaconExecutionPayloadV2 {
            payload_inner: payload_inner.into(),
            withdrawals: withdrawals.clone(),
        }
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_payload_v2 {
    use super::*;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &ExecutionPayloadV2,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BeaconExecutionPayloadV2::from(payload_attributes).serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<ExecutionPayloadV2, D::Error>
    where
        D: Deserializer<'de>,
    {
        BeaconExecutionPayloadV2::deserialize(deserializer).map(Into::into)
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
struct BeaconExecutionPayloadV3<'a> {
    /// Inner V1 payload
    #[serde(flatten)]
    payload_inner: BeaconExecutionPayloadV2<'a>,
    #[serde_as(as = "DisplayFromStr")]
    blob_gas_used: u64,
    #[serde_as(as = "DisplayFromStr")]
    excess_blob_gas: u64,
}

impl<'a> From<BeaconExecutionPayloadV3<'a>> for ExecutionPayloadV3 {
    fn from(payload: BeaconExecutionPayloadV3<'a>) -> Self {
        let BeaconExecutionPayloadV3 { payload_inner, blob_gas_used, excess_blob_gas } = payload;
        ExecutionPayloadV3 { payload_inner: payload_inner.into(), blob_gas_used, excess_blob_gas }
    }
}

impl<'a> From<&'a ExecutionPayloadV3> for BeaconExecutionPayloadV3<'a> {
    fn from(value: &'a ExecutionPayloadV3) -> Self {
        let ExecutionPayloadV3 { payload_inner, blob_gas_used, excess_blob_gas } = value;
        BeaconExecutionPayloadV3 {
            payload_inner: payload_inner.into(),
            blob_gas_used: *blob_gas_used,
            excess_blob_gas: *excess_blob_gas,
        }
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_payload_v3 {
    use super::*;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &ExecutionPayloadV3,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BeaconExecutionPayloadV3::from(payload_attributes).serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<ExecutionPayloadV3, D::Error>
    where
        D: Deserializer<'de>,
    {
        BeaconExecutionPayloadV3::deserialize(deserializer).map(Into::into)
    }
}

/// Represents all possible payload versions.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum BeaconExecutionPayload<'a> {
    /// V1 payload
    V1(BeaconExecutionPayloadV1<'a>),
    /// V2 payload
    V2(BeaconExecutionPayloadV2<'a>),
    /// V3 payload
    V3(BeaconExecutionPayloadV3<'a>),
}

// Deserializes untagged ExecutionPayload by trying each variant in falling order
impl<'de> Deserialize<'de> for BeaconExecutionPayload<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum BeaconExecutionPayloadDesc<'a> {
            V3(BeaconExecutionPayloadV3<'a>),
            V2(BeaconExecutionPayloadV2<'a>),
            V1(BeaconExecutionPayloadV1<'a>),
        }
        match BeaconExecutionPayloadDesc::deserialize(deserializer)? {
            BeaconExecutionPayloadDesc::V3(payload) => Ok(Self::V3(payload)),
            BeaconExecutionPayloadDesc::V2(payload) => Ok(Self::V2(payload)),
            BeaconExecutionPayloadDesc::V1(payload) => Ok(Self::V1(payload)),
        }
    }
}

impl<'a> From<BeaconExecutionPayload<'a>> for ExecutionPayload {
    fn from(payload: BeaconExecutionPayload<'a>) -> Self {
        match payload {
            BeaconExecutionPayload::V1(payload) => {
                ExecutionPayload::V1(ExecutionPayloadV1::from(payload))
            }
            BeaconExecutionPayload::V2(payload) => {
                ExecutionPayload::V2(ExecutionPayloadV2::from(payload))
            }
            BeaconExecutionPayload::V3(payload) => {
                ExecutionPayload::V3(ExecutionPayloadV3::from(payload))
            }
        }
    }
}

impl<'a> From<&'a ExecutionPayload> for BeaconExecutionPayload<'a> {
    fn from(value: &'a ExecutionPayload) -> Self {
        match value {
            ExecutionPayload::V1(payload) => {
                BeaconExecutionPayload::V1(BeaconExecutionPayloadV1::from(payload))
            }
            ExecutionPayload::V2(payload) => {
                BeaconExecutionPayload::V2(BeaconExecutionPayloadV2::from(payload))
            }
            ExecutionPayload::V3(payload) => {
                BeaconExecutionPayload::V3(BeaconExecutionPayloadV3::from(payload))
            }
        }
    }
}

impl<'a> SerializeAs<ExecutionPayload> for BeaconExecutionPayload<'a> {
    fn serialize_as<S>(source: &ExecutionPayload, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        beacon_payload::serialize(source, serializer)
    }
}

impl<'de> DeserializeAs<'de, ExecutionPayload> for BeaconExecutionPayload<'de> {
    fn deserialize_as<D>(deserializer: D) -> Result<ExecutionPayload, D::Error>
    where
        D: Deserializer<'de>,
    {
        beacon_payload::deserialize(deserializer)
    }
}

pub mod beacon_payload {
    use super::*;

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(
        payload_attributes: &ExecutionPayload,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        BeaconExecutionPayload::from(payload_attributes).serialize(serializer)
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<ExecutionPayload, D::Error>
    where
        D: Deserializer<'de>,
    {
        BeaconExecutionPayload::deserialize(deserializer).map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_get_payload_header_response() {
        let s = r#"{"version":"bellatrix","data":{"message":{"header":{"parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","receipts_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prev_randao":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_number":"1","gas_limit":"1","gas_used":"1","timestamp":"1","extra_data":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","base_fee_per_gas":"1","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","transactions_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"},"value":"1","pubkey":"0x93247f2209abcacf57b75a51dafae777f9dd38bc7053d1af526f220a7489a6d3a2753e5f3e8b1cfe39b56f43611df74a"},"signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}}"#;
        let resp: GetExecutionPayloadHeaderResponse = serde_json::from_str(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(resp).unwrap());
    }

    #[test]
    fn serde_payload_header() {
        let s = r#"{"parent_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","fee_recipient":"0xabcf8e0d4e9587369b2301d0790347320302cc09","state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","receipts_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","logs_bloom":"0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","prev_randao":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","block_number":"1","gas_limit":"1","gas_used":"1","timestamp":"1","extra_data":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","base_fee_per_gas":"1","block_hash":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2","transactions_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}"#;
        let header: ExecutionPayloadHeader = serde_json::from_str(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(header).unwrap());
    }
}
