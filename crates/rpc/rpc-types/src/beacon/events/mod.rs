//! Support for the Beacon API events
//!
//! See also [ethereum-beacon-API eventstream](https://ethereum.github.io/beacon-APIs/#/Events/eventstream)

use crate::engine::PayloadAttributes;
use alloy_primitives::{Address, Bytes, B256};
use attestation::AttestationData;
use light_client_finality::LightClientFinalityData;
use light_client_optimistic::LightClientOptimisticData;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

pub mod attestation;
pub mod light_client_finality;
pub mod light_client_optimistic;

/// Topic variant for the eventstream API
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeaconNodeEventTopic {
    PayloadAttributes,
    Head,
    Block,
    Attestation,
    VoluntaryExit,
    BlsToExecutionChange,
    FinalizedCheckpoint,
    ChainReorg,
    ContributionAndProof,
    LightClientFinalityUpdate,
    LightClientOptimisticUpdate,
    BlobSidecar,
}

impl BeaconNodeEventTopic {
    /// Returns the identifier value for the eventstream query
    pub fn query_value(&self) -> &'static str {
        match self {
            BeaconNodeEventTopic::PayloadAttributes => "payload_attributes",
            BeaconNodeEventTopic::Head => "head",
            BeaconNodeEventTopic::Block => "block",
            BeaconNodeEventTopic::Attestation => "attestation",
            BeaconNodeEventTopic::VoluntaryExit => "voluntary_exit",
            BeaconNodeEventTopic::BlsToExecutionChange => "bls_to_execution_change",
            BeaconNodeEventTopic::FinalizedCheckpoint => "finalized_checkpoint",
            BeaconNodeEventTopic::ChainReorg => "chain_reorg",
            BeaconNodeEventTopic::ContributionAndProof => "contribution_and_proof",
            BeaconNodeEventTopic::LightClientFinalityUpdate => "light_client_finality_update",
            BeaconNodeEventTopic::LightClientOptimisticUpdate => "light_client_optimistic_update",
            BeaconNodeEventTopic::BlobSidecar => "blob_sidecar",
        }
    }
}

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

/// Event for the `Head` topic of the beacon API node event stream.
///
/// The node has finished processing, resulting in a new head. previous_duty_dependent_root is
/// \`get_block_root_at_slot(state, compute_start_slot_at_epoch(epoch - 1) - 1)\` and
/// current_duty_dependent_root is \`get_block_root_at_slot(state,
/// compute_start_slot_at_epoch(epoch)
/// - 1)\`. Both dependent roots use the genesis block root in the case of underflow.
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadEvent {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    pub block: B256,
    pub state: B256,
    pub epoch_transition: bool,
    pub previous_duty_dependent_root: B256,
    pub current_duty_dependent_root: B256,
    pub execution_optimistic: bool,
}

/// Event for the `Block` topic of the beacon API node event stream.
///
/// The node has received a valid block (from P2P or API)
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEvent {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    pub block: B256,
    pub execution_optimistic: bool,
}

/// Event for the `Attestation` topic of the beacon API node event stream.
///
/// The node has received a valid attestation (from P2P or API)
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttestationEvent {
    pub aggregation_bits: Bytes,
    pub signature: Bytes,
    pub data: AttestationData,
}

/// Event for the `VoluntaryExit` topic of the beacon API node event stream.
///
/// The node has received a valid voluntary exit (from P2P or API)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoluntaryExitEvent {
    pub message: VoluntaryExitMessage,
    pub signature: Bytes,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoluntaryExitMessage {
    #[serde_as(as = "DisplayFromStr")]
    pub epoch: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub validator_index: u64,
}

/// Event for the `BlsToExecutionChange` topic of the beacon API node event stream.
///
/// The node has received a BLS to execution change (from P2P or API)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsToExecutionChangeEvent {
    pub message: BlsToExecutionChangeMessage,
    pub signature: Bytes,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlsToExecutionChangeMessage {
    #[serde_as(as = "DisplayFromStr")]
    pub validator_index: u64,
    pub from_bls_pubkey: String,
    pub to_execution_address: Address,
}

/// Event for the `Deposit` topic of the beacon API node event stream.
///
/// Finalized checkpoint has been updated
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizedCheckpointEvent {
    pub block: B256,
    pub state: B256,
    #[serde_as(as = "DisplayFromStr")]
    pub epoch: u64,
    pub execution_optimistic: bool,
}

/// Event for the `ChainReorg` topic of the beacon API node event stream.
///
/// The node has reorganized its chain
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainReorgEvent {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub depth: u64,
    pub old_head_block: B256,
    pub new_head_block: B256,
    pub old_head_state: B256,
    pub new_head_state: B256,
    #[serde_as(as = "DisplayFromStr")]
    pub epoch: u64,
    pub execution_optimistic: bool,
}

/// Event for the `ContributionAndProof` topic of the beacon API node event stream.
///
/// The node has received a valid sync committee SignedContributionAndProof (from P2P or API)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionAndProofEvent {
    pub message: ContributionAndProofMessage,
    pub signature: Bytes,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContributionAndProofMessage {
    #[serde_as(as = "DisplayFromStr")]
    pub aggregator_index: u64,
    pub contribution: Contribution,
    pub selection_proof: Bytes,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    pub beacon_block_root: B256,
    #[serde_as(as = "DisplayFromStr")]
    pub subcommittee_index: u64,
    pub aggregation_bits: Bytes,
    pub signature: Bytes,
}

/// Event for the `LightClientFinalityUpdate` topic of the beacon API node event stream.
///
/// The node's latest known `LightClientFinalityUpdate` has been updated
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightClientFinalityUpdateEvent {
    pub version: String,
    pub data: LightClientFinalityData,
}

/// Event for the `LightClientOptimisticUpdate` topic of the beacon API node event stream.
///
/// The node's latest known `LightClientOptimisticUpdate` has been updated
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LightClientOptimisticUpdateEvent {
    pub version: String,
    pub data: LightClientOptimisticData,
}

/// Event for the `BlobSidecar` topic of the beacon API node event stream.
///
/// The node has received a BlobSidecar (from P2P or API) that passes all gossip validations on the
/// blob_sidecar_{subnet_id} topic
#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobSidecarEvent {
    pub block_root: B256,
    #[serde_as(as = "DisplayFromStr")]
    pub index: u64,
    #[serde_as(as = "DisplayFromStr")]
    pub slot: u64,
    pub kzg_commitment: Bytes,
    pub versioned_hash: B256,
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
    /// the execution block number of the parent block.
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
    #[serde(with = "crate::beacon::payload::beacon_api_payload_attributes")]
    pub payload_attributes: PayloadAttributes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_payload_attributes_event() {
        let s = r#"{"version":"capella","data":{"proposal_slot":"173332","proposer_index":"649112","parent_block_root":"0x5a49069647f6bf8f25d76b55ce920947654ade4ba1c6ab826d16712dd62b42bf","parent_block_number":"161093","parent_block_hash":"0x608b3d140ecb5bbcd0019711ac3704ece7be8e6d100816a55db440c1bcbb0251","payload_attributes":{"timestamp":"1697982384","prev_randao":"0x3142abd98055871ebf78f0f8e758fd3a04df3b6e34d12d09114f37a737f8f01e","suggested_fee_recipient":"0x0000000000000000000000000000000000000001","withdrawals":[{"index":"2461612","validator_index":"853570","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"45016211"},{"index":"2461613","validator_index":"853571","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5269785"},{"index":"2461614","validator_index":"853572","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5275106"},{"index":"2461615","validator_index":"853573","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5235962"},{"index":"2461616","validator_index":"853574","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5252171"},{"index":"2461617","validator_index":"853575","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5221319"},{"index":"2461618","validator_index":"853576","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5260879"},{"index":"2461619","validator_index":"853577","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5285244"},{"index":"2461620","validator_index":"853578","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5266681"},{"index":"2461621","validator_index":"853579","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5271322"},{"index":"2461622","validator_index":"853580","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5231327"},{"index":"2461623","validator_index":"853581","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5276761"},{"index":"2461624","validator_index":"853582","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5246244"},{"index":"2461625","validator_index":"853583","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5261011"},{"index":"2461626","validator_index":"853584","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5276477"},{"index":"2461627","validator_index":"853585","address":"0x778f5f13c4be78a3a4d7141bcb26999702f407cf","amount":"5275319"}]}}}"#;

        let event: PayloadAttributesEvent =
            serde_json::from_str::<PayloadAttributesEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }
    #[test]
    fn serde_head_event() {
        let s = r#"{"slot":"10", "block":"0x9a2fefd2fdb57f74993c7780ea5b9030d2897b615b89f808011ca5aebed54eaf", "state":"0x600e852a08c1200654ddf11025f1ceacb3c2e74bdd5c630cde0838b2591b69f9", "epoch_transition":false, "previous_duty_dependent_root":"0x5e0043f107cb57913498fbf2f99ff55e730bf1e151f02f221e977c91a90a0e91", "current_duty_dependent_root":"0x5e0043f107cb57913498fbf2f99ff55e730bf1e151f02f221e977c91a90a0e91", "execution_optimistic": false}"#;

        let event: HeadEvent = serde_json::from_str::<HeadEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_block_event() {
        let s = r#"{"slot":"10", "block":"0x9a2fefd2fdb57f74993c7780ea5b9030d2897b615b89f808011ca5aebed54eaf", "execution_optimistic": false}"#;

        let event: BlockEvent = serde_json::from_str::<BlockEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }
    #[test]
    fn serde_attestation_event() {
        let s = r#"{"aggregation_bits":"0x01", "signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505", "data":{"slot":"1", "index":"1", "beacon_block_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "source":{"epoch":"1", "root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}, "target":{"epoch":"1", "root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}}}"#;

        let event: AttestationEvent = serde_json::from_str::<AttestationEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_voluntary_exit_event() {
        let s = r#"{"message":{"epoch":"1", "validator_index":"1"}, "signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}"#;

        let event: VoluntaryExitEvent = serde_json::from_str::<VoluntaryExitEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_bls_to_execution_change_event() {
        let s = r#"{"message":{"validator_index":"1", "from_bls_pubkey":"0x933ad9491b62059dd065b560d256d8957a8c402cc6e8d8ee7290ae11e8f7329267a8811c397529dac52ae1342ba58c95", "to_execution_address":"0x9be8d619c56699667c1fedcd15f6b14d8b067f72"}, "signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}"#;

        let event: BlsToExecutionChangeEvent =
            serde_json::from_str::<BlsToExecutionChangeEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_finalize_checkpoint_event() {
        let s = r#"{"block":"0x9a2fefd2fdb57f74993c7780ea5b9030d2897b615b89f808011ca5aebed54eaf", "state":"0x600e852a08c1200654ddf11025f1ceacb3c2e74bdd5c630cde0838b2591b69f9", "epoch":"2", "execution_optimistic": false }"#;

        let event: FinalizedCheckpointEvent =
            serde_json::from_str::<FinalizedCheckpointEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_chain_reorg_event() {
        let s = r#"{"slot":"200", "depth":"50", "old_head_block":"0x9a2fefd2fdb57f74993c7780ea5b9030d2897b615b89f808011ca5aebed54eaf", "new_head_block":"0x76262e91970d375a19bfe8a867288d7b9cde43c8635f598d93d39d041706fc76", "old_head_state":"0x9a2fefd2fdb57f74993c7780ea5b9030d2897b615b89f808011ca5aebed54eaf", "new_head_state":"0x600e852a08c1200654ddf11025f1ceacb3c2e74bdd5c630cde0838b2591b69f9", "epoch":"2", "execution_optimistic": false}"#;

        let event: ChainReorgEvent = serde_json::from_str::<ChainReorgEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_contribution_and_proof_event() {
        let s = r#"{"message": {"aggregator_index": "997", "contribution": {"slot": "168097", "beacon_block_root": "0x56f1fd4262c08fa81e27621c370e187e621a67fc80fe42340b07519f84b42ea1", "subcommittee_index": "0", "aggregation_bits": "0xffffffffffffffffffffffffffffffff", "signature": "0x85ab9018e14963026476fdf784cc674da144b3dbdb47516185438768774f077d882087b90ad642469902e782a8b43eed0cfc1b862aa9a473b54c98d860424a702297b4b648f3f30bdaae8a8b7627d10d04cb96a2cc8376af3e54a9aa0c8145e3"}, "selection_proof": "0x87c305f04bfe5db27c2b19fc23e00d7ac496ec7d3e759cbfdd1035cb8cf6caaa17a36a95a08ba78c282725e7b66a76820ca4eb333822bd399ceeb9807a0f2926c67ce67cfe06a0b0006838203b493505a8457eb79913ce1a3bcd1cc8e4ef30ed"}, "signature": "0xac118511474a94f857300b315c50585c32a713e4452e26a6bb98cdb619936370f126ed3b6bb64469259ee92e69791d9e12d324ce6fd90081680ce72f39d85d50b0ff977260a8667465e613362c6d6e6e745e1f9323ec1d6f16041c4e358839ac"}"#;

        let event: ContributionAndProofEvent =
            serde_json::from_str::<ContributionAndProofEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_light_client_finality_update_event() {
        let s = r#"{"version":"phase0", "data": {"attested_header": {"beacon": {"slot":"1", "proposer_index":"1", "parent_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "body_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}}, "finalized_header": {"beacon": {"slot":"1", "proposer_index":"1", "parent_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "body_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}}, "finality_branch": ["0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"], "sync_aggregate": {"sync_committee_bits":"0x01", "sync_committee_signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}, "signature_slot":"1"}}"#;

        let event: LightClientFinalityUpdateEvent =
            serde_json::from_str::<LightClientFinalityUpdateEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }
    #[test]
    fn serde_light_client_optimistic_update_event() {
        let s = r#"{"version":"phase0", "data": {"attested_header": {"beacon": {"slot":"1", "proposer_index":"1", "parent_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "state_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "body_root":"0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}}, "sync_aggregate": {"sync_committee_bits":"0x01", "sync_committee_signature":"0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505"}, "signature_slot":"1"}}"#;

        let event: LightClientOptimisticUpdateEvent =
            serde_json::from_str::<LightClientOptimisticUpdateEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }

    #[test]
    fn serde_blob_sidecar_event() {
        let s = r#"{"block_root": "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2", "index": "1", "slot": "1", "kzg_commitment": "0x1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505cc411d61252fb6cb3fa0017b679f8bb2305b26a285fa2737f175668d0dff91cc1b66ac1fb663c9bc59509846d6ec05345bd908eda73e670af888da41af171505", "versioned_hash": "0xcf8e0d4e9587369b2301d0790347320302cc0943d5a1884560367e8208d920f2"}"#;

        let event: BlobSidecarEvent = serde_json::from_str::<BlobSidecarEvent>(s).unwrap();
        let input = serde_json::from_str::<serde_json::Value>(s).unwrap();
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(input, json);
    }
}
