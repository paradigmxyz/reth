use crate::ScrollTrieAccount;
use alloy_primitives::{B256, U256};
use reth_scroll_primitives::poseidon::{
    field_element_from_be_bytes, hash_with_domain, split_and_hash_be_bytes, FieldElementBytes, Fr,
    PrimeField, DOMAIN_MULTIPLIER_PER_FIELD_ELEMENT,
};

/// An implementation of a value hasher that uses Poseidon.
///
/// This hash provides hashing of a [`ScrollTrieAccount`] and a storage entry ([`U256`]).
#[derive(Debug)]
pub struct PoseidonValueHasher;

impl PoseidonValueHasher {
    /// The number of field elements in the account hashing.
    const ACCOUNT_HASHING_FIELD_ELEMENTS: u64 = 5;

    /// The domain for hashing the account.
    const ACCOUNT_HASHING_DOMAIN: Fr = Fr::from_raw([
        Self::ACCOUNT_HASHING_FIELD_ELEMENTS * DOMAIN_MULTIPLIER_PER_FIELD_ELEMENT,
        0,
        0,
        0,
    ]);

    /// Hashes the account using Poseidon hash function.
    pub(crate) fn hash_account(account: ScrollTrieAccount) -> B256 {
        // combine nonce and code size and parse into field element
        let nonce_code_size_bytes = field_element_from_be_bytes(
            // TODO(scroll): Replace with native handling of bytes instead of using U256.
            U256::from_limbs([account.nonce, account.code_size, 0, 0]).to_be_bytes(),
        );

        // parse remaining field elements
        let balance = field_element_from_be_bytes(account.balance.to_be_bytes());
        let keccak_code_hash = split_and_hash_be_bytes(account.code_hash);
        let storage_root = field_element_from_be_bytes(account.storage_root.0);
        let poseidon_code_hash = field_element_from_be_bytes(account.poseidon_code_hash.0);

        // hash field elements
        let digest_1 =
            hash_with_domain(&[nonce_code_size_bytes, balance], Self::ACCOUNT_HASHING_DOMAIN);
        let digest_2 =
            hash_with_domain(&[storage_root, keccak_code_hash], Self::ACCOUNT_HASHING_DOMAIN);
        let digest = hash_with_domain(&[digest_1, digest_2], Self::ACCOUNT_HASHING_DOMAIN);
        let digest = hash_with_domain(&[digest, poseidon_code_hash], Self::ACCOUNT_HASHING_DOMAIN);

        digest.to_repr().into()
    }

    /// Hashes the storage entry using Poseidon hash function.
    pub(crate) fn hash_storage(entry: U256) -> B256 {
        split_and_hash_be_bytes::<FieldElementBytes>(entry.to_be_bytes()).to_repr().into()
    }
}
