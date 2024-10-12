//! Helper macros for implementing traits for various [`StateProvider`](crate::StateProvider)
//! implementations

/// A macro that delegates trait implementations to the `as_ref` function of the type.
///
/// Used to implement provider traits.
macro_rules! delegate_impls_to_as_ref {
    (for $target:ty => $($trait:ident $(where [$($generics:tt)*])? {  $(fn $func:ident$(<$($generic_arg:ident: $generic_arg_ty:path),*>)?(&self, $($arg:ident: $argty:ty),*) -> $ret:path;)* })* ) => {

        $(
          impl<'a, $($($generics)*)?> $trait for $target {
              $(
                  fn $func$(<$($generic_arg: $generic_arg_ty),*>)?(&self, $($arg: $argty),*) -> $ret {
                    self.as_ref().$func($($arg),*)
                  }
              )*
          }
        )*
    };
}

pub(crate) use delegate_impls_to_as_ref;

/// Delegates the provider trait implementations to the `as_ref` function of the type:
///
/// [`AccountReader`](crate::AccountReader)
/// [`BlockHashReader`](crate::BlockHashReader)
/// [`StateProvider`](crate::StateProvider)
macro_rules! delegate_provider_impls {
    ($target:ty $(where [$($generics:tt)*])?) => {
        $crate::providers::state::macros::delegate_impls_to_as_ref!(
            for $target =>
            AccountReader $(where [$($generics)*])? {
                fn basic_account(&self, address: alloy_primitives::Address) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::Account>>;
            }
            BlockHashReader $(where [$($generics)*])? {
                fn block_hash(&self, number: u64) -> reth_storage_errors::provider::ProviderResult<Option<alloy_primitives::B256>>;
                fn canonical_hashes_range(&self, start: alloy_primitives::BlockNumber, end: alloy_primitives::BlockNumber) -> reth_storage_errors::provider::ProviderResult<Vec<alloy_primitives::B256>>;
            }
            StateProvider $(where [$($generics)*])? {
                fn storage(&self, account: alloy_primitives::Address, storage_key: alloy_primitives::StorageKey) -> reth_storage_errors::provider::ProviderResult<Option<alloy_primitives::StorageValue>>;
                fn bytecode_by_hash(&self, code_hash: alloy_primitives::B256) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::Bytecode>>;
            }
            StateRootProvider $(where [$($generics)*])? {
                fn state_root(&self, state: reth_trie::HashedPostState) -> reth_storage_errors::provider::ProviderResult<alloy_primitives::B256>;
                fn state_root_from_nodes(&self, input: reth_trie::TrieInput) -> reth_storage_errors::provider::ProviderResult<alloy_primitives::B256>;
                fn state_root_with_updates(&self, state: reth_trie::HashedPostState) -> reth_storage_errors::provider::ProviderResult<(alloy_primitives::B256, reth_trie::updates::TrieUpdates)>;
                fn state_root_from_nodes_with_updates(&self, input: reth_trie::TrieInput) -> reth_storage_errors::provider::ProviderResult<(alloy_primitives::B256, reth_trie::updates::TrieUpdates)>;
            }
            StorageRootProvider $(where [$($generics)*])? {
                fn storage_root(&self, address: alloy_primitives::Address, storage: reth_trie::HashedStorage) -> reth_storage_errors::provider::ProviderResult<alloy_primitives::B256>;
                fn storage_proof(&self, address: alloy_primitives::Address, slot: alloy_primitives::B256, storage: reth_trie::HashedStorage) -> reth_storage_errors::provider::ProviderResult<reth_trie::StorageProof>;
            }
            StateProofProvider $(where [$($generics)*])? {
                fn proof(&self, input: reth_trie::TrieInput, address: alloy_primitives::Address, slots: &[alloy_primitives::B256]) -> reth_storage_errors::provider::ProviderResult<reth_trie::AccountProof>;
                fn multiproof(&self, input: reth_trie::TrieInput, targets: alloy_primitives::map::HashMap<alloy_primitives::B256, alloy_primitives::map::HashSet<alloy_primitives::B256>>) -> reth_storage_errors::provider::ProviderResult<reth_trie::MultiProof>;
                fn witness(&self, input: reth_trie::TrieInput, target: reth_trie::HashedPostState) -> reth_storage_errors::provider::ProviderResult<alloy_primitives::map::HashMap<alloy_primitives::B256, alloy_primitives::Bytes>>;
            }
        );
    }
}

pub(crate) use delegate_provider_impls;
