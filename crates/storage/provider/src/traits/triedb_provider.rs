use crate::providers::TrieDBProvider;

/// `TrieDB` provider factory.
///
/// This trait provides access to the `TrieDB` provider
pub trait TrieDBProviderFactory {
    /// Returns a reference to the `TrieDB` provider.
    fn triedb_provider(&self) -> &TrieDBProvider;
}
