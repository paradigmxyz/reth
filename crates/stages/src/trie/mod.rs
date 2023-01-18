#![allow(missing_docs, dead_code, unused_variables, unused_imports)]
use std::{borrow::Borrow, collections::HashMap, marker::PhantomData};

use crate::Transaction;
use bytes::BytesMut;
use hash256_std_hasher::Hash256StdHasher;
use hash_db::{AsHashDB, Prefix};
use memory_db::{HashKey, MemoryDB};
use reference_trie::ReferenceNodeCodec;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    models::AccountBeforeTx,
    table::{Decode, Encode, Table},
    tables,
    transaction::DbTx,
};
use reth_primitives::{
    keccak256, proofs::KeccakHasher, rpc::H160, Account, Address, Bytes, StorageEntry, H256,
    KECCAK_EMPTY, U256,
};
use reth_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use trie_db::{
    node::{NodePlan, Value},
    CError, ChildReference, HashDB, Hasher, NodeCodec, TrieDBMut, TrieDBMutBuilder, TrieLayout,
    TrieMut,
};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub(crate) enum TrieError {
    #[error("{0:?}")]
    ImplError(#[from] Box<trie_db::TrieError<reth_primitives::H256, parity_scale_codec::Error>>),
    #[error("{0:?}")]
    DecodeError(#[from] reth_db::Error),
}

struct DBTrieLayout;

impl TrieLayout for DBTrieLayout {
    const USE_EXTENSION: bool = true;

    // TODO: modify?
    const ALLOW_EMPTY: bool = false;
    // I think non-inlined nodes aren't supported
    const MAX_INLINE_VALUE: Option<u32> = None;

    type Hash = KeccakHasher;
    type Codec = RLPNodeCodec<Self::Hash>;
}

// pub struct HashDatabase<DB: Database> {
//     db: Arc<DB>,
// }

// impl<H: Hasher, DB: Database, T> HashDB<H, T> for HashDatabase<DB> {
//     fn get(&self, key: &H::Out, prefix: Prefix<'_>) -> Option<T> {
//         todo!()
//     }

//     fn contains(&self, key: &H::Out, prefix: Prefix<'_>) -> bool {
//         todo!()
//     }

//     fn insert(&mut self, prefix: Prefix<'_>, value: &[u8]) -> H::Out {
//         todo!()
//     }

//     fn emplace(&mut self, key: H::Out, prefix: Prefix<'_>, value: T) {
//         todo!()
//     }

//     fn remove(&mut self, key: &H::Out, prefix: Prefix<'_>) {
//         todo!()
//     }
// }

// impl<H: Hasher, T, DB: Database> AsHashDB<H, T> for HashDatabase<DB> {
//     fn as_hash_db(&self) -> &dyn HashDB<H, T> {
//         self
//     }

//     fn as_hash_db_mut<'a>(&'a mut self) -> &'a mut (dyn HashDB<H, T> + 'a) {
//         self
//     }
// }

#[derive(Debug, Default, Clone)]
struct RLPNodeCodec<H: Hasher>(PhantomData<H>);

impl<H> NodeCodec for RLPNodeCodec<H>
where
    H: Hasher,
{
    type Error = TrieError;

    type HashOut = <H as Hasher>::Out;

    fn hashed_null_node() -> <H as Hasher>::Out {
        H::hash(<Self as NodeCodec>::empty_node())
    }

    fn decode_plan(data: &[u8]) -> Result<NodePlan, Self::Error> {
        // Self::decode_plan_inner_hashed(data)
        todo!()
    }

    fn is_empty_node(data: &[u8]) -> bool {
        data == <Self as NodeCodec>::empty_node()
    }

    fn empty_node() -> &'static [u8] {
        // rlp('')
        &[0x80]
    }

    fn leaf_node(
        partial: impl Iterator<Item = u8>,
        number_nibble: usize,
        value: Value<'_>,
    ) -> Vec<u8> {
        let contains_hash = matches!(&value, Value::Node(..));
        let mut output: Vec<u8> = Vec::new();
        // let mut output = if contains_hash {
        //     partial_from_iterator_encode(partial, number_nibble, NodeKind::HashedValueLeaf)
        // } else {
        //     partial_from_iterator_encode(partial, number_nibble, NodeKind::Leaf)
        // };
        // match value {
        //     Value::Inline(value) => {
        //         debug_assert!(value.len() < H::LENGTH);
        //         Compact(value.len() as u32).encode_to(&mut output);
        //         output.extend_from_slice(value);
        //     }
        //     Value::Node(hash) => {
        //         debug_assert!(hash.len() == H::LENGTH);
        //         output.extend_from_slice(hash);
        //     }
        // }
        output
    }

    fn extension_node(
        partial: impl Iterator<Item = u8>,
        number_nibble: usize,
        child: ChildReference<Self::HashOut>,
    ) -> Vec<u8> {
        // let mut output = partial_from_iterator_to_key(
        //     partial,
        //     number_nibble,
        //     EXTENSION_NODE_OFFSET,
        //     EXTENSION_NODE_OVER,
        // );
        // match child {
        //     ChildReference::Hash(h) => h.as_ref().encode_to(&mut output),
        //     ChildReference::Inline(inline_data, len) => {
        //         (&AsRef::<[u8]>::as_ref(&inline_data)[..len]).encode_to(&mut output)
        //     }
        // };
        // output
        Vec::new()
    }

    fn branch_node(
        children: impl Iterator<Item = impl Borrow<Option<ChildReference<Self::HashOut>>>>,
        maybe_value: Option<Value<'_>>,
    ) -> Vec<u8> {
        // let mut output = vec![0; BITMAP_LENGTH + 1];
        // let mut prefix: [u8; 3] = [0; 3];
        // let have_value = match maybe_value {
        //     Some(Value::Inline(value)) => {
        //         Compact(value.len() as u32).encode_to(&mut output);
        //         output.extend_from_slice(value);
        //         true
        //     }
        //     None => false,
        //     _ => unimplemented!("unsupported"),
        // };
        // let has_children = children.map(|maybe_child| match maybe_child.borrow() {
        //     Some(ChildReference::Hash(h)) => {
        //         h.as_ref().encode_to(&mut output);
        //         true
        //     }
        //     &Some(ChildReference::Inline(inline_data, len)) => {
        //         inline_data.as_ref()[..len].encode_to(&mut output);
        //         true
        //     }
        //     None => false,
        // });
        // branch_node_buffered(have_value, has_children, prefix.as_mut());
        // output[0..BITMAP_LENGTH + 1].copy_from_slice(prefix.as_ref());
        // output
        Vec::new()
    }

    fn branch_node_nibbled(
        partial: impl Iterator<Item = u8>,
        number_nibble: usize,
        children: impl Iterator<Item = impl Borrow<Option<ChildReference<<H as Hasher>::Out>>>>,
        value: Option<Value<'_>>,
    ) -> Vec<u8> {
        // let contains_hash = matches!(&value, Some(Value::Node(..)));
        // let mut output = match (&value, contains_hash) {
        //     (&None, _) => {
        //         partial_from_iterator_encode(partial, number_nibble, NodeKind::BranchNoValue)
        //     }
        //     (_, false) => {
        //         partial_from_iterator_encode(partial, number_nibble, NodeKind::BranchWithValue)
        //     }
        //     (_, true) => {
        //         partial_from_iterator_encode(partial, number_nibble, NodeKind::HashedValueBranch)
        //     }
        // };

        // let bitmap_index = output.len();
        // let mut bitmap: [u8; BITMAP_LENGTH] = [0; BITMAP_LENGTH];
        // (0..BITMAP_LENGTH).for_each(|_| output.push(0));
        // match value {
        //     Some(Value::Inline(value)) => {
        //         Compact(value.len() as u32).encode_to(&mut output);
        //         output.extend_from_slice(value);
        //     }
        //     Some(Value::Node(hash)) => {
        //         debug_assert!(hash.len() == H::LENGTH);
        //         output.extend_from_slice(hash);
        //     }
        //     None => (),
        // }
        // Bitmap::encode(
        //     children.map(|maybe_child| match maybe_child.borrow() {
        //         Some(ChildReference::Hash(h)) => {
        //             h.as_ref().encode_to(&mut output);
        //             true
        //         }
        //         &Some(ChildReference::Inline(inline_data, len)) => {
        //             inline_data.as_ref()[..len].encode_to(&mut output);
        //             true
        //         }
        //         None => false,
        //     }),
        //     bitmap.as_mut(),
        // );
        // output[bitmap_index..bitmap_index + BITMAP_LENGTH]
        //     .copy_from_slice(&bitmap[..BITMAP_LENGTH]);
        // output
        Vec::new()
    }
}

/// An Ethereum account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
struct EthAccount {
    /// Account nonce.
    nonce: u64,
    /// Account balance.
    balance: U256,
    /// Account's storage root.
    storage_root: H256,
    /// Hash of the account's bytecode.
    code_hash: H256,
}

impl From<Account> for EthAccount {
    fn from(acc: Account) -> Self {
        EthAccount {
            nonce: acc.nonce,
            balance: acc.balance,
            storage_root: H256::zero(),
            code_hash: acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

#[derive(Debug)]
struct DBTrieLoader;

impl DBTrieLoader {
    // Result<H256>
    pub(crate) fn calculate_root<DB: Database>(&mut self, tx: &Transaction<'_, DB>) -> H256 {
        let mut accounts_cursor = tx.cursor_read::<tables::PlainAccountState>().unwrap();
        let mut walker = accounts_cursor.walk(Address::zero()).unwrap();
        // let trie_cursor = tx.cursor_read::<tables::AccountsTrie>().unwrap();

        let mut db = MemoryDB::<KeccakHasher, HashKey<KeccakHasher>, Vec<u8>>::default();
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        while let Some((address, account)) = walker.next().transpose().unwrap() {
            let mut key = EthAccount::from(account);

            // storage_root
            key.storage_root = self.calculate_storage_root(tx, address);

            let mut bytes = BytesMut::new();
            Encodable::encode(&key, &mut bytes);
            trie.insert(address.as_bytes(), &bytes).unwrap();
        }

        *trie.root()
    }

    // Result<H256>
    fn calculate_storage_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        address: Address,
    ) -> H256 {
        let mut db = MemoryDB::<KeccakHasher, HashKey<KeccakHasher>, Vec<u8>>::default();
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>().unwrap();
        let mut walker = storage_cursor.walk_dup(address, H256::zero()).unwrap();

        while let Some((_, StorageEntry { key: storage_key, value })) =
            walker.next().transpose().unwrap()
        {
            let mut bytes = BytesMut::new();
            let location = [H256::from(address).as_bytes(), storage_key.as_bytes()].concat();
            Encodable::encode(&value, &mut bytes);
            trie.insert(location.as_slice(), &bytes).unwrap();
        }

        *trie.root()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, WriteMap},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::{hex_literal::hex, proofs::EMPTY_ROOT, Address, ChainSpec, KECCAK_EMPTY};
    use reth_staged_sync::utils::chainspec::chain_spec_value_parser;
    use std::str::FromStr;
    use trie_db::TrieDBMutBuilder;

    #[test]
    fn empty_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();
        assert_eq!(trie.calculate_root(&tx), EMPTY_ROOT);
    }

    #[test]
    fn verify_genesis() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();
        let ChainSpec { genesis, .. } = chain_spec_value_parser("mainnet").unwrap();

        // Insert account state
        for (address, account) in &genesis.alloc {
            tx.put::<tables::PlainAccountState>(
                *address,
                Account {
                    nonce: account.nonce.unwrap_or_default(),
                    balance: account.balance,
                    bytecode_hash: None,
                },
            )
            .unwrap();
        }

        assert_eq!(trie.calculate_root(&tx), genesis.state_root);
    }
}
