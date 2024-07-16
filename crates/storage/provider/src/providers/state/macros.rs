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
                fn basic_account(&self, address: reth_primitives::Address) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::Account>>;
            }
            BlockHashReader $(where [$($generics)*])? {
                fn block_hash(&self, number: u64) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::B256>>;
                fn canonical_hashes_range(&self, start: reth_primitives::BlockNumber, end: reth_primitives::BlockNumber) -> reth_storage_errors::provider::ProviderResult<Vec<reth_primitives::B256>>;
            }
            StateProvider $(where [$($generics)*])? {
                fn storage(&self, account: reth_primitives::Address, storage_key: reth_primitives::StorageKey) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::StorageValue>>;
                fn bytecode_by_hash(&self, code_hash: reth_primitives::B256) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives::Bytecode>>;
            }
            StateRootProvider $(where [$($generics)*])? {
                fn state_root(&self, state: &revm::db::BundleState) -> reth_storage_errors::provider::ProviderResult<reth_primitives::B256>;
                fn hashed_state_root(&self, state: &reth_trie::HashedPostState) -> reth_storage_errors::provider::ProviderResult<reth_primitives::B256>;
                fn state_root_with_updates(&self, state: &revm::db::BundleState) -> reth_storage_errors::provider::ProviderResult<(reth_primitives::B256, reth_trie::updates::TrieUpdates)>;
                fn hashed_state_root_with_updates(&self, state: &reth_trie::HashedPostState) -> reth_storage_errors::provider::ProviderResult<(reth_primitives::B256, reth_trie::updates::TrieUpdates)>;
            }
            StateProofProvider $(where [$($generics)*])? {
                fn proof(&self, state: &revm::db::BundleState, address: reth_primitives::Address, slots: &[reth_primitives::B256]) -> reth_storage_errors::provider::ProviderResult<reth_trie::AccountProof>;
                fn hashed_proof(&self, state: &reth_trie::HashedPostState, address: reth_primitives::Address, slots: &[reth_primitives::B256]) -> reth_storage_errors::provider::ProviderResult<reth_trie::AccountProof>;
            }
        );
    }
}

pub(crate) use delegate_provider_impls;
