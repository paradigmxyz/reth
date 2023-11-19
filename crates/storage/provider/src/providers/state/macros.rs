//! Helper macros for implementing traits for various [StateProvider](crate::StateProvider)
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
/// [AccountReader](crate::AccountReader)
/// [BlockHashReader](crate::BlockHashReader)
/// [StateProvider](crate::StateProvider)
macro_rules! delegate_provider_impls {
    ($target:ty $(where [$($generics:tt)*])?) => {
        $crate::providers::state::macros::delegate_impls_to_as_ref!(
            for $target =>
            StateRootProvider $(where [$($generics)*])? {
                fn state_root(&self, state: &crate::BundleStateWithReceipts) -> reth_interfaces::provider::ProviderResult<reth_primitives::B256>;
                fn state_root_with_updates(&self, state: &crate::BundleStateWithReceipts) -> reth_interfaces::provider::ProviderResult<(reth_primitives::B256, reth_trie::updates::TrieUpdates)>;
            }
            AccountReader $(where [$($generics)*])? {
                fn basic_account(&self, address: reth_primitives::Address) -> reth_interfaces::provider::ProviderResult<Option<reth_primitives::Account>>;
            }
            BlockHashReader $(where [$($generics)*])? {
                fn block_hash(&self, number: u64) -> reth_interfaces::provider::ProviderResult<Option<reth_primitives::B256>>;
                fn canonical_hashes_range(&self, start: reth_primitives::BlockNumber, end: reth_primitives::BlockNumber) -> reth_interfaces::provider::ProviderResult<Vec<reth_primitives::B256>>;
            }
            StateProvider $(where [$($generics)*])?{
                fn storage(&self, account: reth_primitives::Address, storage_key: reth_primitives::StorageKey) -> reth_interfaces::provider::ProviderResult<Option<reth_primitives::StorageValue>>;
                fn proof(&self, address: reth_primitives::Address, keys: &[reth_primitives::B256]) -> reth_interfaces::provider::ProviderResult<reth_primitives::trie::AccountProof>;
                fn bytecode_by_hash(&self, code_hash: reth_primitives::B256) -> reth_interfaces::provider::ProviderResult<Option<reth_primitives::Bytecode>>;
            }
        );
    }
}

pub(crate) use delegate_provider_impls;
