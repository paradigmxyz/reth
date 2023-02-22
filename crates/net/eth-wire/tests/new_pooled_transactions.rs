//! Decoding tests for [`NewPooledTransactions`]
use reth_eth_wire::NewPooledTransactionHashes66;
use reth_primitives::hex;
use reth_rlp::Decodable;
use std::{fs, path::PathBuf};

#[test]
fn decode_new_pooled_transaction_hashes_network() {
    let network_data_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/new_pooled_transactions_network_rlp");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = NewPooledTransactionHashes66::decode(&mut &hex_data[..]).unwrap();
}
