//! Decoding tests for [`PooledTransactions`]
use alloy_rlp::{Decodable, Encodable};
use reth_eth_wire::{EthVersion, PooledTransactions, ProtocolMessage};
use reth_primitives::{hex, Bytes, PooledTransactionsElement};
use std::{fs, path::PathBuf};
use test_fuzz::test_fuzz;

/// Helper function to ensure encode-decode roundtrip works for [`PooledTransactions`].
#[test_fuzz]
fn roundtrip_pooled_transactions(hex_data: Vec<u8>) -> Result<(), alloy_rlp::Error> {
    let input_rlp = &mut &hex_data[..];
    let txs = match PooledTransactions::decode(input_rlp) {
        Ok(txs) => txs,
        Err(e) => return Err(e),
    };

    // get the amount of bytes decoded in `decode` by subtracting the length of the original buf,
    // from the length of the remaining bytes
    let decoded_len = hex_data.len() - input_rlp.len();
    let expected_encoding = hex_data[..decoded_len].to_vec();

    // do a roundtrip test
    let mut buf = Vec::new();
    txs.encode(&mut buf);
    assert_eq!(expected_encoding, buf);

    // now do another decoding, on what we encoded - this should succeed
    let txs2 = PooledTransactions::decode(&mut &buf[..]).unwrap();

    // ensure that the payload length is the same
    assert_eq!(txs.length(), txs2.length());

    // ensure that the length is equal to the length of the encoded data
    assert_eq!(txs.length(), buf.len());

    Ok(())
}

#[test]
fn decode_pooled_transactions_data() {
    let network_data_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/pooled_transactions_with_blob");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).expect("Unable to decode hex");
    assert!(roundtrip_pooled_transactions(hex_data).is_ok());
}

#[test]
fn decode_request_pair_pooled_blob_transactions() {
    let network_data_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/request_pair_pooled_blob_transactions");
    let data = fs::read_to_string(network_data_path).expect("Unable to read file");
    let hex_data = hex::decode(data.trim()).unwrap();
    let _txs = ProtocolMessage::decode_message(EthVersion::Eth68, &mut &hex_data[..]).unwrap();
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
