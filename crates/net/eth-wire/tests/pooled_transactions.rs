//! Decoding tests for [`PooledTransactions`]
use reth_eth_wire::PooledTransactions;
use reth_primitives::{hex, Bytes, PooledTransactionsElement};
use reth_rlp::Decodable;
use std::{fs, path::PathBuf};

#[test]
fn decode_pooled_transactions_data() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/pooled_transactions_with_blob");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = PooledTransactions::decode(&mut &hex_data[..]).unwrap();
}

#[test]
fn decode_blob_transaction_data() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/blob_transaction");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = PooledTransactionsElement::decode(&mut &hex_data[..]).unwrap();
}

#[test]
fn decode_blob_rpc_transaction() {
    // test data pulled from hive test that sends blob transactions
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/rpc_blob_transaction");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = Bytes::from(hex::decode(data.trim()).unwrap());
    let _txs = PooledTransactionsElement::decode_enveloped(hex_data).unwrap();
}
