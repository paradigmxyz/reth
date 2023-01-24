use crate::Transaction;
use hash_db::{AsHashDB, HashDB, Prefix};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    database::Database,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    proofs::{KeccakHasher, EMPTY_ROOT},
    Account, StorageEntry, StorageTrieEntry, H256, KECCAK_EMPTY, U256,
};
use reth_rlp::{encode_iter, Encodable, RlpDecodable, RlpEncodable};
use std::{borrow::Borrow, marker::PhantomData};
use tracing::*;
use trie_db::{
    node::{NodePlan, Value},
    ChildReference, Hasher, NodeCodec, TrieDBMut, TrieDBMutBuilder, TrieLayout, TrieMut,
};

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub(crate) enum TrieError {
    // TODO: decompose into various different errors
    #[error("Some error occurred")]
    InternalError,
    #[error("The root node wasn't found in the DB")]
    MissingRoot(H256),
    #[error("{0:?}")]
    DatabaseError(#[from] reth_db::Error),
}

struct DBTrieLayout;

impl TrieLayout for DBTrieLayout {
    const USE_EXTENSION: bool = true;

    const ALLOW_EMPTY: bool = false;
    const MAX_INLINE_VALUE: Option<u32> = None;

    type Hash = KeccakHasher;
    type Codec = RLPNodeCodec<Self::Hash>;
}

/// Database wrapper implementing HashDB trait.
struct HashDatabase<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
}

impl<'tx, 'itx, DB: Database> HashDatabase<'tx, 'itx, DB> {
    fn new(tx: &'tx Transaction<'itx, DB>) -> Result<Self, TrieError> {
        let root = RLPNodeCodec::<KeccakHasher>::hashed_null_node();
        if tx.get::<tables::AccountsTrie>(root)?.is_none() {
            tx.put::<tables::AccountsTrie>(
                root,
                RLPNodeCodec::<KeccakHasher>::empty_node().to_vec(),
            )?;
        }
        Ok(Self { tx })
    }

    #[allow(dead_code)]
    fn new_with_root(tx: &'tx Transaction<'itx, DB>, root: H256) -> Result<Self, TrieError> {
        tx.get::<tables::AccountsTrie>(root)?.ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx })
    }
}

impl<H, DB, V> HashDB<H, V> for HashDatabase<'_, '_, DB>
where
    H: Hasher,
    DB: Database,
    V: From<Vec<u8>>,
    H::Out: Into<H256>,
{
    fn get(&self, key: &H::Out, _prefix: Prefix<'_>) -> Option<V> {
        // TODO: see if there's a better way to transmit database errors
        match self.tx.get::<tables::AccountsTrie>((*key).into()) {
            Ok(v) => v.map(V::from),
            Err(e) => {
                error!(target: "sync::trie::db", ?key, ?e, "Error while fetching value from Database");
                None
            }
        }
    }

    fn contains(&self, key: &H::Out, prefix: Prefix<'_>) -> bool {
        <Self as HashDB<H, V>>::get(self, key, prefix).is_some()
    }

    fn insert(&mut self, _prefix: Prefix<'_>, value: &[u8]) -> H::Out {
        let hash = H::hash(value);
        if let Err(e) = self.tx.put::<tables::AccountsTrie>(hash.into(), value.to_vec()) {
            error!(target: "sync::trie::db", ?value, ?e, "Error while inserting value in Database");
            panic!("failed to insert value in Database")
        }
        hash
    }

    fn emplace(&mut self, _key: H::Out, _prefix: Prefix<'_>, _value: V) {
        todo!()
    }

    fn remove(&mut self, key: &H::Out, _prefix: Prefix<'_>) {
        let _ = self.tx.delete::<tables::AccountsTrie>((*key).into(), None);
    }
}

impl<H: Hasher, DB: Database, T> AsHashDB<H, T> for HashDatabase<'_, '_, DB>
where
    H: Hasher,
    DB: Database,
    T: From<Vec<u8>>,
    H::Out: Into<H256>,
{
    fn as_hash_db(&self) -> &dyn HashDB<H, T> {
        self
    }

    fn as_hash_db_mut<'a>(&'a mut self) -> &'a mut (dyn HashDB<H, T> + 'a) {
        self
    }
}

/// Database wrapper implementing HashDB trait.
struct DupHashDatabase<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
    key: H256,
}

impl<'tx, 'itx, DB: Database> DupHashDatabase<'tx, 'itx, DB> {
    fn new(tx: &'tx Transaction<'itx, DB>, key: H256) -> Result<Self, TrieError> {
        let root = RLPNodeCodec::<KeccakHasher>::hashed_null_node();
        let mut cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;
        if cursor.seek_by_key_subkey(key, root)?.is_none() {
            cursor.append_dup(
                key,
                StorageTrieEntry {
                    hash: root,
                    node: RLPNodeCodec::<KeccakHasher>::empty_node().to_vec(),
                },
            )?;
        }
        Ok(Self { tx, key })
    }

    #[allow(dead_code)]
    fn new_with_root(
        tx: &'tx Transaction<'itx, DB>,
        key: H256,
        root: H256,
    ) -> Result<Self, TrieError> {
        tx.cursor_dup_write::<tables::StoragesTrie>()?
            .seek_by_key_subkey(key, root)?
            .ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx, key })
    }
}

impl<H, DB, V> HashDB<H, V> for DupHashDatabase<'_, '_, DB>
where
    H: Hasher,
    DB: Database,
    V: From<Vec<u8>>,
    H::Out: Into<H256>,
{
    fn get(&self, key: &H::Out, _prefix: Prefix<'_>) -> Option<V> {
        // TODO: see if there's a better way to transmit database errors
        let res = self
            .tx
            .cursor_dup_read::<tables::StoragesTrie>()
            .and_then(|mut c| c.seek_by_key_subkey(self.key, (*key).into()));
        match res {
            Ok(v) => v.map(|entry| V::from(entry.node)),
            Err(e) => {
                error!(target: "sync::trie::db", ?key, ?e, "Error while fetching value from Database");
                None
            }
        }
    }

    fn contains(&self, key: &H::Out, prefix: Prefix<'_>) -> bool {
        <Self as HashDB<H, V>>::get(self, key, prefix).is_some()
    }

    fn insert(&mut self, _prefix: Prefix<'_>, value: &[u8]) -> H::Out {
        let hash = H::hash(value);
        let res = self.tx.cursor_dup_write::<tables::StoragesTrie>().and_then(|mut c| {
            c.seek_by_key_subkey(self.key, hash.into())?.map(|_| c.delete_current()).transpose()?;
            c.append_dup(self.key, StorageTrieEntry { hash: hash.into(), node: value.to_vec() })
        });
        if let Err(e) = res {
            error!(target: "sync::trie::db", ?value, ?e, "Error while inserting value in Database");
            panic!("failed to insert value in Database")
        }
        hash
    }

    fn emplace(&mut self, _key: H::Out, _prefix: Prefix<'_>, _value: V) {
        todo!()
    }

    fn remove(&mut self, key: &H::Out, _prefix: Prefix<'_>) {
        let _ = self.tx.cursor_dup_write::<tables::StoragesTrie>().and_then(|mut c| {
            c.seek_by_key_subkey(self.key, (*key).into())?.map(|_| c.delete_current()).transpose()
        });
    }
}

impl<H: Hasher, DB: Database, T> AsHashDB<H, T> for DupHashDatabase<'_, '_, DB>
where
    H: Hasher,
    DB: Database,
    T: From<Vec<u8>>,
    H::Out: Into<H256>,
{
    fn as_hash_db(&self) -> &dyn HashDB<H, T> {
        self
    }

    fn as_hash_db_mut<'a>(&'a mut self) -> &'a mut (dyn HashDB<H, T> + 'a) {
        self
    }
}

// Encodes a partial path in the trie, adding necessary flags.
fn encode_partial(
    mut partial: impl Iterator<Item = u8>,
    nibbles: usize,
    terminating: bool,
) -> Vec<u8> {
    debug_assert_ne!(nibbles, 0);
    let mut out = Vec::with_capacity(nibbles / 2 + 1);

    let mut flag_byte = if terminating { 0x20 } else { 0x00 };

    if nibbles % 2 != 0 {
        flag_byte |= 0x10;
        flag_byte |= match partial.next() {
            Some(v) => v,
            None => unreachable!("partial should be non-empty"),
        };
    }
    out.push(flag_byte);
    out.extend(partial);
    out
}

/// Responsible for encoding/decoding the trie nodes.
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
        if data == Self::empty_node() {
            return Ok(NodePlan::Empty)
        }
        todo!()
    }

    fn is_empty_node(data: &[u8]) -> bool {
        data == <Self as NodeCodec>::empty_node()
    }

    fn empty_node() -> &'static [u8] {
        // rlp('')
        &[reth_rlp::EMPTY_STRING_CODE]
    }

    fn leaf_node(
        partial: impl Iterator<Item = u8>,
        number_nibble: usize,
        value: Value<'_>,
    ) -> Vec<u8> {
        let encoded_vec = encode_partial(partial, number_nibble, true);
        let encoded_partial = encoded_vec.as_ref();
        let value = match value {
            Value::Inline(node) => node,
            Value::Node(hash) => hash,
        };

        let mut output = Vec::new();

        encode_iter([encoded_partial, value].into_iter(), &mut output);
        output
    }

    fn extension_node(
        partial: impl Iterator<Item = u8>,
        number_nibble: usize,
        child: ChildReference<Self::HashOut>,
    ) -> Vec<u8> {
        let encoded_vec = encode_partial(partial, number_nibble, false);
        let encoded_partial = encoded_vec.as_ref();

        let value = match child {
            ChildReference::Hash(ref hash) => hash.as_ref(),
            ChildReference::Inline(ref _inline_data, _len) => {
                unreachable!("can't happen")
            }
        };

        let mut output = Vec::new();
        encode_iter([encoded_partial, value].into_iter(), &mut output);
        output
    }

    fn branch_node(
        children: impl Iterator<Item = impl Borrow<Option<ChildReference<Self::HashOut>>>>,
        maybe_value: Option<Value<'_>>,
    ) -> Vec<u8> {
        let mut output = Vec::new();
        let mut children: Vec<_> = children
            .map(|c| -> Vec<u8> {
                match c.borrow() {
                    Some(ChildReference::Hash(hash)) => hash.as_ref().to_vec(),
                    Some(ChildReference::Inline(_value, _len)) => {
                        unreachable!("can't happen because all keys are equal length");
                    }
                    None => vec![],
                }
            })
            .collect();

        children.push(match maybe_value {
            Some(Value::Inline(value)) => value.to_vec(),
            None => vec![],
            _ => unimplemented!("unsupported"),
        });
        encode_iter(children.iter().map(|c| c.as_slice()), &mut output);
        output
    }

    fn branch_node_nibbled(
        _partial: impl Iterator<Item = u8>,
        _number_nibble: usize,
        _children: impl Iterator<Item = impl Borrow<Option<ChildReference<<H as Hasher>::Out>>>>,
        _value: Option<Value<'_>>,
    ) -> Vec<u8> {
        unimplemented!("unused");
    }
}

/// An Ethereum account, for RLP encoding traits deriving.
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
            storage_root: EMPTY_ROOT,
            code_hash: acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DBTrieLoader;

impl DBTrieLoader {
    /// Calculates the root of the state trie, saving intermediate hashes in the database.
    pub(crate) fn calculate_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
    ) -> Result<H256, TrieError> {
        let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
        let mut walker = accounts_cursor.walk(H256::zero())?;

        let mut db = HashDatabase::new(tx)?;

        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        while let Some((hashed_address, account)) = walker.next().transpose()? {
            let mut value = EthAccount::from(account);

            value.storage_root = self.calculate_storage_root(tx, hashed_address)?;

            let mut out = Vec::new();
            Encodable::encode(&value, &mut out);
            trie.insert(hashed_address.as_bytes(), out.as_ref())
                .map_err(|_| TrieError::InternalError)?;
        }

        Ok(*trie.root())
    }

    fn calculate_storage_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        address: H256,
    ) -> Result<H256, TrieError> {
        let mut db = DupHashDatabase::new(tx, address)?;
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;
        let mut walker = storage_cursor.walk_dup(address, H256::zero())?;

        while let Some((_, StorageEntry { key: storage_key, value })) = walker.next().transpose()? {
            let mut out = Vec::new();
            Encodable::encode(&value, &mut out);
            trie.insert(storage_key.as_bytes(), &out).map_err(|_| TrieError::InternalError)?;
        }

        Ok(*trie.root())
    }
}

// another possible implementation, using cita_trie
/*
fn calculate_root<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(storage: Vec<(K, V)>) -> H256 {
    let memdb = Arc::new(cita_trie::MemoryDB::new(true));
    let hasher = Arc::new(HasherKeccak::new());

    let mut trie = PatriciaTrie::new(Arc::clone(&memdb), Arc::clone(&hasher));

    for (key, value) in storage {
        trie.insert(key.into(), value.into()).unwrap();
    }
    H256::from_slice(trie.root().unwrap().as_slice())
}
*/

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, WriteMap},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::{hex_literal::hex, keccak256, proofs::EMPTY_ROOT, Address, ChainSpec};
    use reth_staged_sync::utils::chainspec::chain_spec_value_parser;
    use std::{collections::HashMap, str::FromStr};
    use triehash::sec_trie_root;

    #[test]
    fn empty_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();
        assert_eq!(trie.calculate_root(&tx), Ok(EMPTY_ROOT));
    }

    #[test]
    fn single_account_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();
        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let account = Account { nonce: 0, balance: U256::MAX, bytecode_hash: None };
        tx.put::<tables::HashedAccount>(keccak256(address), account).unwrap();
        let mut encoded_account = Vec::new();
        EthAccount::from(account).encode(&mut encoded_account);
        assert_eq!(
            trie.calculate_root(&tx),
            Ok(H256(sec_trie_root::<KeccakHasher, _, _, _>([(address, encoded_account)]).0))
        );
    }

    #[test]
    fn two_accounts_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let accounts = [
            (
                Address::from(hex!("9fe4abd71ad081f091bd06dd1c16f7e92927561e")),
                Account { nonce: 155, balance: U256::from(414241124), bytecode_hash: None },
            ),
            (
                Address::from(hex!("f8a6edaad4a332e6e550d0915a7fd5300b0b12d1")),
                Account { nonce: 3, balance: U256::from(78978), bytecode_hash: None },
            ),
        ];
        for (address, account) in accounts.clone() {
            tx.put::<tables::HashedAccount>(keccak256(address), account).unwrap();
        }
        let encoded_accounts = accounts.iter().map(|(k, v)| {
            let mut out = Vec::new();
            EthAccount::from(*v).encode(&mut out);
            (k, out)
        });
        assert_eq!(
            trie.calculate_root(&tx),
            Ok(H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts).0))
        );
    }

    #[test]
    fn single_storage_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let hashed_address = keccak256(address);

        let storage = Vec::from([(H256::from_low_u64_be(2), U256::from(1))]);
        for (k, v) in storage.clone() {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: v },
            )
            .unwrap();
        }
        let encoded_storage = storage.iter().map(|(k, v)| {
            let mut out = Vec::new();
            v.encode(&mut out);
            (k, out)
        });
        assert_eq!(
            trie.calculate_storage_root(&tx, hashed_address),
            Ok(H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0))
        );
    }

    #[test]
    fn single_account_with_storage_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let hashed_address = keccak256(address);

        let storage = HashMap::from([
            (H256::zero(), U256::from(3)),
            (H256::from_low_u64_be(2), U256::from(1)),
        ]);
        let code = "el buen fla";
        let account = Account {
            nonce: 155,
            balance: U256::from(414241124u32),
            bytecode_hash: Some(keccak256(code)),
        };
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();

        for (k, v) in storage.clone() {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: v },
            )
            .unwrap();
        }
        let mut out = Vec::new();
        let mut eth_account = EthAccount::from(account);

        let encoded_storage = storage.iter().map(|(k, v)| {
            let mut out = Vec::new();
            v.encode(&mut out);
            (k, out)
        });

        eth_account.storage_root = H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0);
        eth_account.encode(&mut out);
        assert_eq!(
            trie.calculate_root(&tx),
            Ok(H256(sec_trie_root::<KeccakHasher, _, _, _>([(address, out)]).0))
        );
    }

    #[test]
    fn verify_genesis() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let ChainSpec { genesis, .. } = chain_spec_value_parser("mainnet").unwrap();

        // Insert account state
        for (address, account) in &genesis.alloc {
            tx.put::<tables::HashedAccount>(
                keccak256(address),
                Account {
                    nonce: account.nonce.unwrap_or_default(),
                    balance: account.balance,
                    bytecode_hash: None,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        assert_eq!(trie.calculate_root(&tx), Ok(genesis.state_root));
    }
}
