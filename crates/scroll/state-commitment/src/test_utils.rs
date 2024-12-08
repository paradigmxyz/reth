use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{B256, U256};
use poseidon_bn254::{hash_with_domain, Fr, PrimeField};
use reth_primitives::Account;
use zktrie::HashField;
use zktrie_rust::{db::SimpleDb, hash::AsHash, types::Hashable};

const ACCOUNT_COMPRESSION_FLAG: u32 = 8;
const STORAGE_COMPRESSION_FLAG: u32 = 1;

/// Reverses the ordering of bits in a [`B256`] type.
pub fn b256_reverse_bits(b256: B256) -> B256 {
    let mut b256 = b256.0;
    for byte in &mut b256 {
        *byte = byte.reverse_bits();
    }
    B256::from(b256)
}

/// Clear the last byte of a [`B256`] type.
pub fn b256_clear_last_byte(mut b256: B256) -> B256 {
    // set the largest byte to 0
    <B256 as AsMut<[u8; 32]>>::as_mut(&mut b256)[31] = 0;
    b256
}

/// Clear the first byte of a [`B256`] type.
pub fn b256_clear_first_byte(mut b256: B256) -> B256 {
    // set the smallest byte to 0
    <B256 as AsMut<[u8; 32]>>::as_mut(&mut b256)[0] = 0;
    b256
}

/// Clear the most significant byte of a [`U256`] type.
pub fn u256_clear_msb(mut balance: U256) -> U256 {
    // set the most significant 8 bits to 0
    unsafe {
        balance.as_limbs_mut()[3] &= 0x00FFFFFFFFFFFFFF;
    }
    balance
}

/// Calculates the state root of a set of accounts and their storage entries using the zktrie
/// implementation.
pub fn state_root<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (B256, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let mut trie = zktrie();
    for (address, (account, storage)) in accounts {
        let key = parse_key_as_hash(address);
        let mut account_bytes = Vec::with_capacity(5);

        #[cfg(feature = "scroll")]
        let code_size = account.get_code_size();
        #[cfg(not(feature = "scroll"))]
        let code_size = 0;

        #[cfg(feature = "scroll")]
        let poseidon_code_hash = account.get_poseidon_code_hash();
        #[cfg(not(feature = "scroll"))]
        let poseidon_code_hash = B256::default();

        account_bytes.push(U256::from_limbs([account.nonce, code_size, 0, 0]).to_be_bytes());
        account_bytes.push(account.balance.to_be_bytes());
        account_bytes.push(storage_root(storage).0);
        account_bytes.push(account.bytecode_hash.unwrap_or(KECCAK_EMPTY).0);
        account_bytes.push(poseidon_code_hash.0);

        trie.try_update(&key, ACCOUNT_COMPRESSION_FLAG, account_bytes).unwrap();
    }
    trie.prepare_root().unwrap();
    let root = trie.root().to_bytes();
    B256::from_slice(&root)
}

/// Calculates the storage root of a set of storage entries using the zktrie implementation.
pub fn storage_root<S>(storage: S) -> B256
where
    S: IntoIterator<Item = (B256, U256)>,
{
    let mut storage_trie = zktrie();
    for (key, value) in storage {
        let key = parse_key_as_hash(key);
        storage_trie.try_update(&key, STORAGE_COMPRESSION_FLAG, vec![value.to_be_bytes()]).unwrap();
    }
    storage_trie.prepare_root().unwrap();
    let root = storage_trie.root().to_bytes();
    B256::from_slice(&root)
}

fn parse_key_as_hash(key: B256) -> AsHash<HashField> {
    let mut key = key.0;
    key.reverse();
    AsHash::from_bytes(&key).unwrap()
}

fn poseidon_hash_scheme(a: &[u8; 32], b: &[u8; 32], domain: &[u8; 32]) -> Option<[u8; 32]> {
    let a = Fr::from_repr_vartime(*a)?;
    let b = Fr::from_repr_vartime(*b)?;
    let domain = Fr::from_repr_vartime(*domain)?;
    Some(hash_with_domain(&[a, b], domain).to_repr())
}

fn zktrie() -> zktrie_rust::raw::ZkTrieImpl<AsHash<HashField>, SimpleDb, 248> {
    zktrie::init_hash_scheme_simple(poseidon_hash_scheme);
    zktrie_rust::raw::ZkTrieImpl::<AsHash<HashField>, SimpleDb, 248>::new_zktrie_impl(
        SimpleDb::new(),
    )
    .unwrap()
}
