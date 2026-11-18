//! EIP-8297 vectors from ethereum/execution-specs, as vendored at the pinned pbtree revision.

use alloy_primitives::{hex, keccak256, Address, B256, U256};
use reth_trie_pbt::{key, AccountData, BinaryTree, Blake3Hasher, StateTree};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("binary_trie_vectors.json")).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    hex::decode(value.as_str().unwrap()).unwrap()
}

fn b256(value: &Value) -> B256 {
    value.as_str().unwrap().parse().unwrap()
}

fn u256(value: &Value) -> U256 {
    value.as_str().unwrap().parse().unwrap()
}

#[test]
fn reference_trie_roots_and_proofs() {
    for vector in vectors()["trie_roots"].as_array().unwrap() {
        let mut tree = BinaryTree::<Blake3Hasher>::new();
        for entry in vector["entries"].as_array().unwrap() {
            tree.insert(bytes(&entry["key"]), b256(&entry["value"])).unwrap();
        }
        let root = b256(&vector["root"]);
        assert_eq!(tree.root(), root, "{}", vector["name"]);
        let rebuilt = BinaryTree::<Blake3Hasher>::from_sorted_entries(
            tree.entries().map(|(key, value)| (key.to_vec(), value)),
        )
        .unwrap();
        assert_eq!(rebuilt.root(), root);
        for (key, value) in tree.entries() {
            assert_eq!(tree.proof(key).verify::<Blake3Hasher>(root, key), Ok(Some(value)));
        }
    }
}

#[test]
fn reference_embedding() {
    let vectors = vectors();
    let embedding = &vectors["embedding"];
    let address: Address = embedding["address20"].as_str().unwrap().parse().unwrap();
    assert_eq!(key::address32(address), b256(&embedding["address32"]));
    for (field, sub_index) in [("basic_data_key", 0), ("code_hash_key", 1), ("delegation_key", 2)] {
        assert_eq!(
            key::header_key::<Blake3Hasher>(address, sub_index).as_slice(),
            bytes(&embedding[field])
        );
    }
    for (slot, expected) in embedding["storage_slot_keys"].as_object().unwrap() {
        assert_eq!(
            key::storage_key::<Blake3Hasher>(address, slot.parse().unwrap()),
            bytes(expected)
        );
    }
    let code_hash = b256(&embedding["code_hash"]);
    for (chunk, expected) in embedding["code_chunk_keys"].as_object().unwrap() {
        assert_eq!(
            key::code_key::<Blake3Hasher>(code_hash, chunk.parse().unwrap()).as_slice(),
            bytes(expected)
        );
    }
}

#[test]
fn reference_code_chunks() {
    for vector in vectors()["chunkify_code"].as_array().unwrap() {
        let expected: Vec<_> = vector["chunks"].as_array().unwrap().iter().map(b256).collect();
        assert_eq!(key::chunkify_code(&bytes(&vector["code"])), expected, "{}", vector["name"]);
    }
}

#[test]
fn reference_basic_data() {
    for vector in vectors()["encode_basic_data"].as_array().unwrap() {
        let size = u32::try_from(vector["code_size"].as_u64().unwrap()).unwrap();
        let nonce = vector["nonce"].as_u64().unwrap();
        let encoded = key::encode_basic_data(size, nonce, u256(&vector["balance"])).unwrap();
        assert_eq!(encoded, b256(&vector["encoded"]));
    }
}

#[test]
fn reference_state_roots() {
    for vector in vectors()["pbt_state"].as_array().unwrap() {
        let mut state = StateTree::<Blake3Hasher>::new();
        for (address, account) in vector["accounts"].as_object().unwrap() {
            let address = address.parse().unwrap();
            let code = bytes(&account["code"]);
            assert_eq!(keccak256(&code), b256(&account["code_hash"]));
            state
                .set_account(
                    address,
                    AccountData {
                        nonce: account["nonce"].as_u64().unwrap(),
                        balance: u256(&account["balance"]),
                        code: code.into(),
                    },
                )
                .unwrap();
            for (slot, value) in account["storage"].as_object().unwrap() {
                state.set_storage(address, slot.parse().unwrap(), b256(value)).unwrap();
            }
        }
        assert_eq!(state.root(), b256(&vector["root"]), "{}", vector["name"]);
    }
}
