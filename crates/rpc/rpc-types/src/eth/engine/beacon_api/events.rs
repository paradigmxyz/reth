//! Support for the Beacon API events
//!
//! See also [ethereum-beacon-API eventstream](https://ethereum.github.io/beacon-APIs/#/Events/eventstream)

use crate::engine::PayloadAttributes;
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

/// Event for the `payload_attributes` topic of the beacon API node event stream.
///
/// This event gives block builders and relays sufficient information to construct or verify a block
/// at `proposal_slot`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadAttributesEvent {
    /// the identifier of the beacon hard fork at `proposal_slot`, e.g `"bellatrix"`, `"capella"`.
    pub version: String,
    /// Wrapped data of the event.
    pub data: PayloadAttributesData,
}

impl PayloadAttributesEvent {
    /// Returns the payload attributes
    pub fn attributes(&self) -> &PayloadAttributes {
        &self.data.payload_attributes
    }
}

/// Data of the event that contains the payload attributes
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadAttributesData {
    /// The slot at which a block using these payload attributes may be built
    #[serde_as(as = "DisplayFromStr")]
    pub proposal_slot: u64,
    /// the beacon block root of the parent block to be built upon.
    pub parent_block_root: B256,
    /// he execution block number of the parent block.
    #[serde_as(as = "DisplayFromStr")]
    pub parent_block_number: u64,
    /// the execution block hash of the parent block.
    pub parent_block_hash: B256,
    /// The execution block number of the parent block.
    /// the validator index of the proposer at `proposal_slot` on the chain identified by
    /// `parent_block_root`.
    #[serde_as(as = "DisplayFromStr")]
    pub proposer_index: u64,
    /// Beacon API encoding of `PayloadAttributesV<N>` as defined by the `execution-apis`
    /// specification
    ///
    /// Note: this uses the beacon API format which uses snake-case and quoted decimals rather than
    /// big-endian hex.
    #[serde(with = "crate::eth::engine::payload::beacon_api_payload_attributes")]
    pub payload_attributes: PayloadAttributes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_payload_attributes_event() {
        let s = r#"{"version":"capella","data":{"proposal_slot":"173332","proposer_index":"649112","parent_block_root":"0x5a49069647f6bf8f25d76b55ce920947654ade4ba1c6ab826d16712dd62b42bf","parent_block_number":"161093","parent_block_hash":"0x608b3d140ecb5bbcd0019711ac3704ece7be8e6d100816a55db440c1bcbb0251","payload_attributes":{"timestamp":"1697982384","prev_randao":"0x3142abd98055871ebf78f0f8e758fd3a04df3b6e34d12d09114f37a737f8f01e","suggested_fee_recipient":"0x0000000000000000000000000000000000000001","withdrawals":[{"index":"2461612","validator_index":"853570","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"45016211"},{"index":"2461613","validator_index":"853571","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5269785"},{"index":"2461614","validator_index":"853572","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5275106"},{"index":"2461615","validator_index":"853573","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5235962"},{"index":"2461616","validator_index":"853574","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5252171"},{"index":"2461617","validator_index":"853575","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5221319"},{"index":"2461618","validator_index":"853576","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5260879"},{"index":"2461619","validator_index":"853577","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5285244"},{"index":"2461620","validator_index":"853578","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5266681"},{"index":"2461621","validator_index":"853579","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5271322"},{"index":"2461622","validator_index":"853580","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5231327"},{"index":"2461623","validator_index":"853581","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5276761"},{"index":"2461624","validator_index":"853582","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5246244"},{"index":"2461625","validator_index":"853583","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5261011"},{"index":"2461626","validator_index":"853584","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5276477"},{"index":"2461627","validator_index":"853585","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5275319"}]}}}"#;

        let event = serde_json::from_str::<PayloadAttributesEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }
}
