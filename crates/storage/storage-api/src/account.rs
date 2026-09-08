use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber};
use auto_impl::auto_impl;
use core::ops::{RangeBounds, RangeInclusive};
use reth_db_models::AccountBeforeTx;
use reth_primitives_traits::Account;
use reth_storage_errors::provider::ProviderResult;

/// Account type selected by a state or changeset provider.
#[auto_impl(&, Arc, Box)]
pub trait AccountExtensionProvider {
    /// Chain-specific data carried by every account returned by this provider.
    type AccountExtension: reth_primitives_traits::AccountExtension;
}

/// Account reader
pub trait AccountReader: AccountExtensionProvider {
    /// Get basic account information.
    ///
    /// Returns `None` if the account doesn't exist.
    fn basic_account(
        &self,
        address: &Address,
    ) -> ProviderResult<Option<Account<Self::AccountExtension>>>;
}

/// Account reader
pub trait AccountExtReader: AccountReader {
    /// Iterate over account changesets and return all account address that were changed.
    fn changed_accounts_with_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>>;

    /// Get basic account information for multiple accounts. A more efficient version than calling
    /// [`AccountReader::basic_account`] repeatedly.
    ///
    /// Returns `None` if the account doesn't exist.
    #[expect(clippy::type_complexity)]
    fn basic_accounts(
        &self,
        _iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account<Self::AccountExtension>>)>>;

    /// Iterate over account changesets and return all account addresses that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<BlockNumber>>>;
}

crate::macros::impl_provider_refs!(T: AccountExtReader {
    fn changed_accounts_with_range(
        &self,
        _range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        T::changed_accounts_with_range(&**self, _range)
    }
    fn basic_accounts(
        &self,
        _iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account<Self::AccountExtension>>)>> {
        T::basic_accounts(&**self, _iter)
    }
    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<BlockNumber>>> {
        T::changed_accounts_and_blocks_with_range(&**self, range)
    }
});

/// `AccountChange` reader
pub trait ChangeSetReader: AccountExtensionProvider {
    /// Iterate over account changesets and return the account state from before this block.
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx<Self::AccountExtension>>>;

    /// Search the block's changesets for the given address, and return the result.
    ///
    /// Returns `None` if the account was not changed in this block.
    fn get_account_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx<Self::AccountExtension>>>;

    /// Get all account changesets in a range of blocks.
    ///
    /// Accepts any range type that implements `RangeBounds<BlockNumber>`, including:
    /// - `Range<BlockNumber>` (e.g., `0..100`)
    /// - `RangeInclusive<BlockNumber>` (e.g., `0..=99`)
    /// - `RangeFrom<BlockNumber>` (e.g., `0..`) - iterates until exhausted
    ///
    /// If there is no start bound, 0 is used as the start block.
    ///
    /// Returns a vector of (`block_number`, changeset) pairs.
    fn account_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx<Self::AccountExtension>)>>;
}

crate::macros::impl_provider_refs!(T: ChangeSetReader {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx<Self::AccountExtension>>> {
        T::account_block_changeset(&**self, block_number)
    }
    fn get_account_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx<Self::AccountExtension>>> {
        T::get_account_before_block(&**self, block_number, address)
    }
    fn account_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx<Self::AccountExtension>)>> {
        T::account_changesets_range(&**self, range)
    }
});

crate::macros::impl_provider_refs!(T: AccountReader {
    fn basic_account(
        &self,
        address: &Address,
    ) -> ProviderResult<Option<Account<Self::AccountExtension>>> {
        T::basic_account(&**self, address)
    }
});

/// Rejects protocols whose account encoding cannot represent custom extensions.
///
/// This checks the selected type, not the feature flag or an individual account's value:
/// Ethereum remains supported in builds that also enable custom chains.
pub fn ensure_no_account_extensions<E: reth_primitives_traits::AccountExtension>(
    protocol: &'static str,
) -> ProviderResult<()> {
    if core::any::TypeId::of::<E>() !=
        core::any::TypeId::of::<reth_primitives_traits::EmptyAccountExtension>()
    {
        return Err(reth_storage_errors::provider::ProviderError::AccountExtensionsUnsupported(
            protocol,
        ));
    }
    Ok(())
}
