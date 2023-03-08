use crate::{
    keccak256, Address, Bytes, GenesisAccount, Header, Log, Receipt, TransactionSigned, Withdrawal,
    H256,
};
use bytes::BytesMut;
use hash_db::Hasher;
use hex_literal::hex;
use plain_hasher::PlainHasher;
use reth_codecs::{main_codec, Compact};
use reth_rlp::Encodable;
use std::collections::HashMap;
use triehash::{ordered_trie_root, sec_trie_root};

/// Saves the progress of MerkleStage
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct ProofCheckpoint {
    /// The next hashed account to insert into the trie.
    pub hashed_address: Option<H256>,
    /// The next storage entry to insert into the trie.
    pub storage_key: Option<H256>,
    /// Current intermediate root for `AccountsTrie`.
    pub account_root: Option<H256>,
    /// Current intermediate storage root from an account.
    pub storage_root: Option<H256>,
}

/// Saves the progress of AccountHashing
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from
    pub address: Option<Address>,
    /// Start transition id
    pub from: u64,
    /// Last transition id
    pub to: u64,
}
