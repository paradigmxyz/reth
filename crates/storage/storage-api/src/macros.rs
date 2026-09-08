//! Helper macros for implementing traits for various `StateProvider`
//! implementations

/// A macro that delegates trait implementations to the `as_ref` function of the type.
///
/// Used to implement provider traits.
#[macro_export]
macro_rules! delegate_impls_to_as_ref {
    (for $target:ty => $($trait:ident $(where [$($generics:tt)*])? { $(type $associated:ident = $value:ty;)* $(fn $func:ident$(<$($generic_arg:ident: $generic_arg_ty:path),*>)?(&self, $($arg:ident: $argty:ty),*) -> $ret:path;)* })* ) => {

        $(
          impl<'a, $($($generics)*)?> $trait for $target {
              $(type $associated = $value;)*
              $(
                  fn $func$(<$($generic_arg: $generic_arg_ty),*>)?(&self, $($arg: $argty),*) -> $ret {
                    self.as_ref().$func($($arg),*)
                  }
              )*
          }
        )*
    };
}

pub use delegate_impls_to_as_ref;

/// Delegates the provider trait implementations to the `as_ref` function of the type:
///
/// [`AccountReader`](crate::AccountReader)
/// [`BlockHashReader`](crate::BlockHashReader)
/// [`StateProvider`](crate::StateProvider)
#[macro_export]
macro_rules! delegate_provider_impls {
    ($target:ty, $extension:ty $(where [$($generics:tt)*])?) => {
        $crate::macros::delegate_impls_to_as_ref!(
            for $target =>
            AccountExtensionProvider $(where [$($generics)*])? {
                type AccountExtension = $extension;
            }
            AccountReader $(where [$($generics)*])? {
                fn basic_account(&self, address: &alloy_primitives::Address) -> reth_storage_api::errors::provider::ProviderResult<Option<reth_primitives_traits::Account<Self::AccountExtension>>>;
            }
            BlockHashReader $(where [$($generics)*])? {
                fn block_hash(&self, number: u64) -> reth_storage_api::errors::provider::ProviderResult<Option<alloy_primitives::B256>>;
                fn canonical_hashes_range(&self, start: alloy_primitives::BlockNumber, end: alloy_primitives::BlockNumber) -> reth_storage_api::errors::provider::ProviderResult<Vec<alloy_primitives::B256>>;
            }
            StateProvider $(where [$($generics)*])? {
                fn storage(&self, account: alloy_primitives::Address, storage_key: alloy_primitives::StorageKey) -> reth_storage_api::errors::provider::ProviderResult<Option<alloy_primitives::StorageValue>>;
            }
            BytecodeReader $(where [$($generics)*])? {
                fn bytecode_by_hash(&self, code_hash: &alloy_primitives::B256) -> reth_storage_api::errors::provider::ProviderResult<Option<reth_primitives_traits::Bytecode>>;
            }
            StateRootProvider $(where [$($generics)*])? {
                fn state_root(&self, state: reth_trie::HashedPostState<Self::AccountExtension>) -> reth_storage_api::errors::provider::ProviderResult<alloy_primitives::B256>;
                fn state_root_from_nodes(&self, input: reth_trie::TrieInput<Self::AccountExtension>) -> reth_storage_api::errors::provider::ProviderResult<alloy_primitives::B256>;
                fn state_root_with_updates(&self, state: reth_trie::HashedPostState<Self::AccountExtension>) -> reth_storage_api::errors::provider::ProviderResult<(alloy_primitives::B256, reth_trie::updates::TrieUpdates)>;
                fn state_root_from_nodes_with_updates(&self, input: reth_trie::TrieInput<Self::AccountExtension>) -> reth_storage_api::errors::provider::ProviderResult<(alloy_primitives::B256, reth_trie::updates::TrieUpdates)>;
            }
            StorageRootProvider $(where [$($generics)*])? {
                fn storage_root(&self, address: alloy_primitives::Address, storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<alloy_primitives::B256>;
                fn storage_proof(&self, address: alloy_primitives::Address, slot: alloy_primitives::B256, storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::StorageProof>;
                fn storage_multiproof(&self, address: alloy_primitives::Address, slots: &[alloy_primitives::B256], storage: reth_trie::HashedStorage) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::StorageMultiProof>;
            }
            StateProofProvider $(where [$($generics)*])? {
                fn proof(&self, input: reth_trie::TrieInput<Self::AccountExtension>, address: alloy_primitives::Address, slots: &[alloy_primitives::B256]) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::AccountProof<Self::AccountExtension>>;
                fn multiproof(&self, input: reth_trie::TrieInput<Self::AccountExtension>, targets: reth_trie::MultiProofTargets) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::MultiProof>;
                fn multiproof_v2(&self, input: reth_trie::TrieInput<Self::AccountExtension>, targets: reth_trie::MultiProofTargetsV2) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::DecodedMultiProofV2>;
                fn witness(&self, input: reth_trie::TrieInput<Self::AccountExtension>, target: reth_trie::HashedPostState<Self::AccountExtension>, mode: reth_trie::ExecutionWitnessMode) -> reth_storage_api::errors::provider::ProviderResult<Vec<alloy_primitives::Bytes>>;
            }
            HashedPostStateProvider $(where [$($generics)*])? {
                fn hashed_post_state(&self, bundle_state: &revm::database::BundleState) -> reth_storage_api::errors::provider::ProviderResult<reth_trie::HashedPostState<Self::AccountExtension>>;
            }
        );
    }
}

pub use delegate_provider_impls;

// Explicit forwarding preserves the inherited account-type equality; auto_impl's
// additional supertrait bounds can hide it from associated-type normalization.
macro_rules! impl_provider_refs {
    ($ty:ident: $trait:ident $(, shared_bounds = [$($bound:path),*])? { $($body:tt)* }) => {
        impl<$ty: $trait + ?Sized $($(+ $bound)*)?> $trait for &$ty { $($body)* }
        impl<$ty: $trait + ?Sized> $trait for alloc::boxed::Box<$ty> { $($body)* }
        impl<$ty: $trait + ?Sized $($(+ $bound)*)?> $trait for alloc::sync::Arc<$ty> { $($body)* }
    };
}
pub(crate) use impl_provider_refs;
