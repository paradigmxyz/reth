use alloy_rlp::Decodable;
use reth_eth_wire::{ClayerConsensusMessage, ClayerConsensusMessageHeader};
use reth_primitives::B256;
use reth_provider::{BlockReaderIdExt, ConsensusNumberReader, ConsensusNumberWriter};
use reth_rpc_types::PeerId;
use tracing::info;

use crate::{
    consensus::{ParsedMessage, PbftError, PbftState},
    ClayerConsensusEngine,
};

#[derive(Debug)]
pub enum ConsensusEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    PeerMessage(PeerId, ClayerConsensusMessage),
    BlockValid(B256),
    BlockInvalid(B256),
    BlockCommit((B256, u64, bool)),
}

pub fn parse_consensus_message(
    bytes: &reth_primitives::Bytes,
) -> Result<ClayerConsensusMessage, PbftError> {
    ClayerConsensusMessage::decode(&mut bytes.to_vec().as_slice()).map_err(|err| {
        PbftError::SerializationError(
            "Error parsing ClayerConsensusMessage message".into(),
            err.to_string(),
        )
    })
}

fn parse_consensus_message_header(
    bytes: &reth_primitives::Bytes,
) -> Result<ClayerConsensusMessageHeader, PbftError> {
    ClayerConsensusMessageHeader::decode(&mut bytes.to_vec().as_slice()).map_err(|err| {
        PbftError::SerializationError(
            "Error parsing ClayerConsensusMessageHeader message".into(),
            err.to_string(),
        )
    })
}

pub fn handle_consensus_event<Client, CDB>(
    consensus: &mut ClayerConsensusEngine<Client, CDB>,
    incoming_event: ConsensusEvent,
    state: &mut PbftState,
) -> Result<bool, PbftError>
where
    CDB: ConsensusNumberReader + ConsensusNumberWriter + 'static,
    Client: BlockReaderIdExt + 'static,
{
    match incoming_event {
        ConsensusEvent::BlockValid(block_id) => consensus.on_block_valid(block_id, state)?,
        ConsensusEvent::BlockInvalid(block_id) => consensus.on_block_invalid(block_id)?,
        ConsensusEvent::BlockCommit((block_id, timestamp, committing)) => {
            consensus.on_block_commit(block_id, timestamp, committing, state)?
        }
        ConsensusEvent::PeerMessage(peer_id, message) => {
            let header: ClayerConsensusMessageHeader =
                parse_consensus_message_header(&message.header_bytes)?;
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
            consensus.on_peer_message(peer_id, parsed_message, state)?
        }
        ConsensusEvent::PeerConnected(peer_id) => {
            info!(target: "consensus::cl","Received PeerConnected message with peer ID: {:?}", peer_id);
            consensus.on_peer_connected(peer_id, state)?
        }
        ConsensusEvent::PeerDisconnected(peer_id) => {
            info!(target: "consensus::cl","Received PeerDisconnected message with peer ID: {:?}", peer_id);
        }
    }

    Ok(true)
}
