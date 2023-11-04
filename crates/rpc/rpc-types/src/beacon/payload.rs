#![allow(missing_docs)]
//! Payload support for the beacon API.
//!
//! Internal helper module to deserialize/serialize the payload attributes for the beacon API, which
//! uses snake case and quoted decimals.
//!
//! This is necessary because we don't want to allow a mixture of both formats, hence `serde`
//! aliases are not an option.

pub use crate::Withdrawal;
use crate::{
    eth::{transaction::BlobTransactionSidecar, withdrawal::BeaconAPIWithdrawal},
    ExecutionPayloadV1,
};
use alloy_primitives::{Address, Bloom, Bytes, B256, B64, U256, U64};
use c_kzg::{Blob, Bytes48};
use serde::{ser::SerializeMap, Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DisplayFromStr};

#[derive(Serialize, Deserialize)]
pub(crate) struct BeaconExecutionPayloadV1 {
    pub(crate) parent_hash: B256,
    pub(crate) fee_recipient: Address,
    pub(crate) state_root: B256,
    pub(crate) receipts_root: B256,
    pub(crate) logs_bloom: Bloom,
    pub(crate) prev_randao: B256,
    pub(crate) block_number: U64,
    pub(crate) gas_limit: U64,
    pub(crate) gas_used: U64,
    pub(crate) timestamp: U64,
    pub(crate) extra_data: Bytes,
    pub(crate) base_fee_per_gas: U256,
    pub(crate) block_hash: B256,
    pub(crate) transactions: Vec<Bytes>,
}

impl From<BeaconExecutionPayloadV1> for ExecutionPayloadV1 {
    fn from(payload: BeaconExecutionPayloadV1) -> Self {
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
        }
    }
}

/// A helper serde module to convert from/to the Beacon API which uses quoted decimals rather than
/// big-endian hex.
pub mod beacon_payload_v1 {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Serialize the payload attributes for the beacon API.
    pub fn serialize<S>(payload_attributes: &Withdrawal, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        todo!()
    }

    /// Deserialize the payload attributes for the beacon API.
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Withdrawal, D::Error>
    where
        D: Deserializer<'de>,
    {
        todo!()
    }
}
