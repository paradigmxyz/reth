//! Traits and default implementations related to retrieval of blinded trie nodes.

use alloy_primitives::{Bytes, B256};
use reth_execution_errors::SparseTrieError;
use reth_trie_common::Nibbles;

/// Factory for instantiating blinded node providers.
pub trait BlindedProviderFactory {
    /// Type capable of fetching blinded account nodes.
    type AccountNodeProvider: BlindedProvider;
    /// Type capable of fetching blinded storage nodes.
    type StorageNodeProvider: BlindedProvider;

    /// Returns blinded account node provider.
    fn account_node_provider(&self) -> Self::AccountNodeProvider;

    /// Returns blinded storage node provider.
    fn storage_node_provider(&self, account: B256) -> Self::StorageNodeProvider;
}

/// Trie node provider for retrieving blinded nodes.
pub trait BlindedProvider {
    /// The error type for the provider.
    type Error: Into<SparseTrieError>;

    /// Retrieve blinded node by path.
    fn blinded_node(&mut self, path: &Nibbles) -> Result<Option<Bytes>, Self::Error>;
}

/// Default blinded node provider factory that creates [`DefaultBlindedProvider`].
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct DefaultBlindedProviderFactory;

impl BlindedProviderFactory for DefaultBlindedProviderFactory {
    type AccountNodeProvider = DefaultBlindedProvider;
    type StorageNodeProvider = DefaultBlindedProvider;

    fn account_node_provider(&self) -> Self::AccountNodeProvider {
        DefaultBlindedProvider
    }

    fn storage_node_provider(&self, _account: B256) -> Self::StorageNodeProvider {
        DefaultBlindedProvider
    }
}

/// Default blinded node provider that always returns `Ok(None)`.
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct DefaultBlindedProvider;

impl BlindedProvider for DefaultBlindedProvider {
    type Error = SparseTrieError;

    fn blinded_node(&mut self, _path: &Nibbles) -> Result<Option<Bytes>, Self::Error> {
        Ok(None)
    }
}

/// Right pad the path with 0s and return as [`B256`].
#[inline]
pub fn pad_path_to_key(path: &Nibbles) -> B256 {
    let mut padded = path.pack();
    padded.resize(32, 0);
    B256::from_slice(&padded)
}
