//! Helper macros for implementing traits for various [StateProvider](crate::StateProvider)
//! implementations

/// A macro that delegates trait implementations to the `as_ref` function of the type.
///
/// Used to implement provider traits.
macro_rules! delegate_impls_to_as_ref {
    ( for $target:ty => $($trait:ident {  $(fn $func:ident(&self, $($arg:ident: $argty:ty),*) -> $ret:ty;)* })* ) => {

        $(
          impl<'a, TX: DbTx<'a>> $trait for $target {
              $(
                  fn $func(&self, $($arg: $argty),*) -> $ret {
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
/// [AccountProvider](crate::AccountProvider)
/// [BlockHashProvider](crate::BlockHashProvider)
/// [StateProvider](crate::StateProvider)
macro_rules! delegate_provider_impls {
    ($target:ty) => {
        $crate::providers::state::macros::delegate_impls_to_as_ref!(
            for $target =>
            AccountProvider {
                fn basic_account(&self, address: Address) -> Result<Option<Account>>;
            }
            BlockHashProvider {
                fn block_hash(&self, number: U256) -> Result<Option<H256>>;
            }
            StateProvider {
                fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>;
                fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>>;
            }
        );
    }
}

pub(crate) use delegate_provider_impls;
