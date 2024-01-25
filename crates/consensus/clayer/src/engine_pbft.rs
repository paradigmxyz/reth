use alloy_primitives::B256;
use alloy_rlp::Decodable;
use reth_eth_wire::{ClayerBlock, ClayerConsensusMessage, ClayerConsensusMessageHeader};
use reth_rpc_types::PeerId;
use tracing::info;

use crate::{
    consensus::{ParsedMessage, PbftConfig, PbftError, PbftState},
    ClayerConsensusEngine,
};

pub struct PbftEngine {
    config: PbftConfig,
}

impl PbftEngine {
    pub fn new(config: PbftConfig) -> Self {
        PbftEngine { config }
    }
}

#[derive(Debug)]
pub enum ConsensusEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    PeerMessage(ClayerConsensusMessage, PeerId),
}

fn handle_consensus_event(
    consensus: &mut ClayerConsensusEngine,
    incoming_event: ConsensusEvent,
    state: &mut PbftState,
) -> Result<bool, PbftError> {
    match incoming_event {
        // ConsensusEvent::BlockNew(block) => consensus.on_block_new(block, state)?,
        // ConsensusEvent::BlockValid(block_id) => consensus.on_block_valid(block_id, state)?,
        // ConsensusEvent::BlockInvalid(block_id) => consensus.on_block_invalid(block_id)?,
        // ConsensusEvent::BlockCommit(block_id) => consensus.on_block_commit(block_id, state)?,
        ConsensusEvent::PeerMessage(message, peer_id) => {
            let header: ClayerConsensusMessageHeader =
                ClayerConsensusMessageHeader::decode(&mut message.header_bytes.to_vec().as_slice())
                    .map_err(|err| {
                        PbftError::SerializationError(
                            "Error parsing header from vote".into(),
                            err.to_string(),
                        )
                    })?;
            let verified_signer_id = header.signer_id.clone();
            let parsed_message = ParsedMessage::from_peer_message(message, state.id.as_slice())?;
            let pbft_signer_id = parsed_message.info().signer_id;
            if pbft_signer_id != verified_signer_id {
                return Err(PbftError::InvalidMessage(format!(
                    "Mismatch between PbftMessage's signer ID ({:?}) and PeerMessage's signer ID \
                     ({:?}) of peer message: {:?}",
                    pbft_signer_id, verified_signer_id, parsed_message
                )));
            }
            consensus.on_peer_message(parsed_message, state)?
        }
        ConsensusEvent::PeerConnected(info) => {
            // info!("Received PeerConnected message with peer info: {:?}", info);
            // node.on_peer_connected(info.peer_id, state)?
        }
        ConsensusEvent::PeerDisconnected(id) => {
            info!(target: "consensus::cl","Received PeerDisconnected for peer ID: {:?}", id);
        }
    }

    Ok(true)
}
