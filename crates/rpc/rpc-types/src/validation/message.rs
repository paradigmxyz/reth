use reth_primitives::{Address,  U256,  H256};
use serde::{Deserialize, Serialize};

/// Message containing the block data to be validated
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub slot: U256,
    pub parent_hash: H256,
    pub block_hash: H256,
    pub builder_pubkey: String,
    pub proposer_pubkey: String,
    pub proposer_fee_recipient: Address,
    pub gas_limit: U256,
    pub gas_used: U256,
    pub value: U256,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGE: &str = include_str!("../../test_data/validation/message.json");

    #[test]
    fn test_deserialize_validation_message() {
        let _message: Message = serde_json::from_str(MESSAGE).unwrap();
    }
}
