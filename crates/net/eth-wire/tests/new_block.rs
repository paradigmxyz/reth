//! Decoding tests for [`NewBlock`]
use alloy_rlp::Decodable;
use reth_eth_wire::NewBlock;
use reth_primitives::hex;
use std::{fs, path::PathBuf};

#[test]
fn decode_new_block_network() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/new_block_network_rlp");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = NewBlock::decode(&mut &hex_data[..]).unwrap();
}

#[test]
fn decode_new_block_network_bsc_one() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/bsc_new_block_network_one");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = NewBlock::decode(&mut &hex_data[..]).unwrap();
}

#[test]
fn decode_new_block_network_bsc_two() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/bsc_new_block_network_two");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = NewBlock::decode(&mut &hex_data[..]).unwrap();
}
