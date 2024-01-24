use super::message::ParsedMessage;
use super::PbftConfig;
use reth_eth_wire::{ClayerBlock, PbftMessageInfo, PbftMessageType};
use reth_primitives::{Block, Bytes, Signature, B256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use tracing::trace;

pub struct PbftLog {
    unvalidated_blocks: HashMap<B256, ClayerBlock>,
    blocks: HashSet<ClayerBlock>,
    messages: HashSet<ParsedMessage>,
    /// Maximum log size
    max_log_size: u64,
}

impl fmt::Display for PbftLog {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let msg_infos: Vec<PbftMessageInfo> =
            self.messages.iter().map(|msg| msg.info().clone()).collect();
        let string_infos: Vec<String> = msg_infos
            .iter()
            .map(|info: &PbftMessageInfo| -> String {
                format!(
                    "    {{ {}, view: {}, seq: {}, signer: {} }}",
                    info.ptype,
                    info.view,
                    info.seq_num,
                    hex::encode(info.signer_id.clone()),
                )
            })
            .collect();

        write!(f, "\nPbftLog:\n{}", string_infos.join("\n"))
    }
}

impl PbftLog {
    /// Create a new, empty `PbftLog`
    pub fn new(config: &PbftConfig) -> Self {
        PbftLog {
            unvalidated_blocks: HashMap::new(),
            blocks: HashSet::new(),
            messages: HashSet::new(),
            max_log_size: config.max_log_size,
        }
    }

    /// Add an already validated `Block` to the log
    pub fn add_validated_block(&mut self, block: ClayerBlock) {
        trace!(target: "consensus::cl", "Adding validated block to log: {:?}", block);
        self.blocks.insert(block);
    }

    /// Add an unvalidated `Block` to the log
    pub fn add_unvalidated_block(&mut self, block: ClayerBlock) {
        trace!(target: "consensus::cl","Adding unvalidated block to log: {:?}", block);
        self.unvalidated_blocks.insert(block.block_id(), block);
    }

    /// Move the `Block` corresponding to `block_id` from `unvalidated_blocks` to `blocks`. Return
    /// the block itself to be used by the calling code.
    pub fn block_validated(&mut self, block_id: B256) -> Option<ClayerBlock> {
        trace!(target: "consensus::cl","Marking block as validated: {:?}", block_id);
        self.unvalidated_blocks.remove(&block_id).map(|block| {
            self.blocks.insert(block.clone());
            block
        })
    }

    /// Drop the `Block` corresponding to `block_id` from `unvalidated_blocks`.
    pub fn block_invalidated(&mut self, block_id: B256) -> bool {
        trace!(target: "consensus::cl","Dropping invalidated block: {:?}", block_id);
        self.unvalidated_blocks.remove(&block_id).is_some()
    }

    /// Get all `Block`s in the message log with the specified block number
    pub fn get_blocks_with_num(&self, block_num: u64) -> Vec<&ClayerBlock> {
        self.blocks.iter().filter(|block| block.block_num() == block_num).collect()
    }

    /// Get the `Block` with the specified block ID
    pub fn get_block_with_id(&self, block_id: B256) -> Option<&ClayerBlock> {
        self.blocks.iter().find(|block| block.block_id() == block_id)
    }

    /// Get the `Block` with the specified block ID from `unvalidated_blocks`.
    pub fn get_unvalidated_block_with_id(&self, block_id: &B256) -> Option<&ClayerBlock> {
        self.unvalidated_blocks.get(block_id)
    }

    /// Add a parsed PBFT message to the log
    pub fn add_message(&mut self, msg: ParsedMessage) {
        trace!(target: "consensus::cl","Adding message to log: {:?}", msg);
        self.messages.insert(msg);
    }

    /// Check if the log has a PrePrepare at the given view and sequence number that matches the
    /// given block ID
    pub fn has_pre_prepare(&self, seq_num: u64, view: u64, block_id: B256) -> bool {
        self.get_messages_of_type_seq_view(PbftMessageType::PrePrepare, seq_num, view)
            .iter()
            .any(|msg| msg.get_block_id() == block_id)
    }

    /// Obtain all messages from the log that match the given type and sequence_number
    pub fn get_messages_of_type_seq(
        &self,
        msg_type: PbftMessageType,
        sequence_number: u64,
    ) -> Vec<&ParsedMessage> {
        self.messages
            .iter()
            .filter(|&msg| {
                let info = (*msg).info();
                info.ptype == msg_type as u8 && info.seq_num == sequence_number
            })
            .collect()
    }

    /// Obtain all messages from the log that match the given type and view
    pub fn get_messages_of_type_view(
        &self,
        msg_type: PbftMessageType,
        view: u64,
    ) -> Vec<&ParsedMessage> {
        self.messages
            .iter()
            .filter(|&msg| {
                let info = (*msg).info();
                info.ptype == msg_type as u8 && info.view == view
            })
            .collect()
    }

    /// Obtain all messages from the log that match the given type, sequence number, and view
    pub fn get_messages_of_type_seq_view(
        &self,
        msg_type: PbftMessageType,
        sequence_number: u64,
        view: u64,
    ) -> Vec<&ParsedMessage> {
        self.messages
            .iter()
            .filter(|&msg| {
                let info = (*msg).info();
                info.ptype == msg_type as u8 && info.seq_num == sequence_number && info.view == view
            })
            .collect()
    }

    /// Obtain all messages from the log that match the given type, sequence number, view, and
    /// block_id
    pub fn get_messages_of_type_seq_view_block(
        &self,
        msg_type: PbftMessageType,
        sequence_number: u64,
        view: u64,
        block_id: B256,
    ) -> Vec<&ParsedMessage> {
        self.messages
            .iter()
            .filter(|&msg| {
                let info = (*msg).info();
                let msg_block_id = (*msg).get_block_id();
                info.ptype == msg_type as u8
                    && info.seq_num == sequence_number
                    && info.view == view
                    && msg_block_id == block_id
            })
            .collect()
    }

    /// Garbage collect the log if it has reached the `max_log_size`
    #[allow(clippy::ptr_arg)]
    pub fn garbage_collect(&mut self, current_seq_num: u64) {
        // If the max log size has been reached, filter out all old messages
        if self.messages.len() as u64 >= self.max_log_size {
            // The node needs to keep messages from the previous sequence number in case it
            // needs to build the next consensus seal
            self.messages.retain(|msg| msg.info().seq_num >= current_seq_num - 1);

            self.blocks.retain(|block| block.block_num() >= current_seq_num - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::{Block, Header};
    use std::hash::Hash;

    #[test]
    fn test_header_hash() {}
}
