use super::{config::PbftConfig, message::ParsedMessage};
use alloy_primitives::{Address, Bytes, B256};
use reth_ecies::util::pk2id;
use reth_eth_wire::{PbftMessage, PbftMessageInfo, PbftMessageType};
use reth_network::config::rng_secret_key;
use reth_primitives::{public_key_to_address, Block};
use reth_rpc_types::PeerId;
use secp256k1::SECP256K1;

/// Create a mock configuration given a number of nodes
pub fn mock_config(num_nodes: u8) -> PbftConfig {
    let mut config = PbftConfig::default();

    for i in 0..num_nodes {
        let secret = rng_secret_key();
        let pk: secp256k1::PublicKey = secret.public_key(SECP256K1);
        let id = pk2id(&pk);

        config.members.push(id);
    }
    config
}

/// Create a Block for the given block number
pub fn mock_block(num: u64) -> Block {
    let mut block = Block::default();
    block.header.number = num;
    block.header.nonce = num;

    block
}

/// Create a PbftMessage
pub fn mock_msg(
    msg_type: PbftMessageType,
    view: u64,
    seq_num: u64,
    signer_id: PeerId,
    block_id: B256,
    from_self: bool,
) -> ParsedMessage {
    let info = PbftMessageInfo { ptype: msg_type as u8, view, seq_num, signer_id };
    let msg = PbftMessage { info, block_id };

    let mut parsed = ParsedMessage::from_pbft_message(msg).expect("Failed to parse PbftMessage");
    parsed.from_self = from_self;
    parsed
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use alloy_primitives::{Bytes, B256};
    use hex::FromHex;
    use reth_ecies::util::pk2id;
    use reth_network::config::rng_secret_key;
    use reth_rpc_types::PeerId;
    use secp256k1::SECP256K1;
    #[test]
    fn test_header_hash() {
        let secret = rng_secret_key();
        let pk = secret.public_key(SECP256K1);
        let peer_id = pk2id(&pk);
        let bytes = Bytes::copy_from_slice(pk.serialize().as_slice());

        let peer_id_str = hex::encode(peer_id);

        println!("peer_id {}", peer_id);
        println!("peer_id_str {}", peer_id_str);
        println!("{}", PeerId::from_str(&peer_id_str).unwrap());
        let ss ="0x4e8a9147b075af967410d1c32c6304e79ef6ab14b6f86700fa9709711ece7ca7395eec4968dedc1e856afd27e5dfbb9f0b109e765b92fa98f0f9b00e221a57e4";
        println!("{}", PeerId::from_str(ss).unwrap())
    }
}
