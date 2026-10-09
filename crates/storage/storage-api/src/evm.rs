//! State access for EVM execution.

use crate::StateProvider;
use alloc::boxed::Box;
use alloy_primitives::{Address, BlockNumber, StorageKey, StorageValue, B256};
use core::ops::Deref;
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_errors::provider::ProviderResult;

/// Provides the state necessary for EVM execution.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait EvmStateProvider: crate::BytecodeReader {
    /// Returns the account, or `None` if it does not exist.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>>;

    /// Returns the block hash, or `None` if the block does not exist.
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>>;

    /// Returns the storage value of the given account and slot.
    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>>;
}

/// Type-erased provider for EVM execution.
pub type EvmStateProviderBox = Box<dyn EvmStateProvider + Send>;

/// Adapts an owned or borrowed full state provider for EVM execution.
#[derive(Debug, Clone, Copy)]
pub struct EvmStateProviderAdapter<P>(pub P);

impl<P> Deref for EvmStateProviderAdapter<P> {
    type Target = P;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<P: StateProvider> EvmStateProvider for EvmStateProviderAdapter<P> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        self.0.basic_account(address)
    }

    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        self.0.block_hash(number)
    }

    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        self.0.storage(account, storage_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{noop::NoopProvider, StateProviderBox};
    use alloc::sync::Arc;

    #[test]
    fn converts_owned_borrowed_and_boxed_state_providers() {
        let provider = NoopProvider::default();
        assert_reads((&provider).into_evm_state_provider());
        assert_reads((&provider as &dyn StateProvider).into_evm_state_provider());
        assert_reads(provider.into_evm_state_provider());

        let boxed = Box::new(NoopProvider::default()) as StateProviderBox;
        assert_reads(boxed.into_evm_state_provider());
        assert_reads(Arc::new(NoopProvider::default()).into_evm_state_provider());
    }

    #[test]
    fn delegates_execution_only_references_and_boxes() {
        let provider = NoopProvider::default();
        let adapter = (&provider).into_evm_state_provider();
        assert_reads(&adapter as &dyn EvmStateProvider);
        assert_reads(Box::new(adapter) as Box<dyn EvmStateProvider + '_>);

        let boxed = Box::new(provider.into_evm_state_provider()) as EvmStateProviderBox;
        assert_reads(boxed);
        assert_reads(Arc::new(NoopProvider::default().into_evm_state_provider()));
    }

    #[test]
    fn exposes_inner_state_provider() {
        let provider = NoopProvider::default();
        let adapter = (&provider).into_evm_state_provider();
        assert_eq!(adapter.account_balance(&Address::ZERO).unwrap(), None);
        assert!(core::ptr::eq(*adapter, &raw const provider));
    }

    fn assert_reads(provider: impl EvmStateProvider) {
        assert_eq!(provider.basic_account(&Address::ZERO).unwrap(), None);
        assert_eq!(provider.block_hash(1).unwrap(), None);
        assert_eq!(provider.bytecode_by_hash(&B256::ZERO).unwrap(), None);
        assert_eq!(provider.storage(Address::ZERO, B256::ZERO).unwrap(), None);
    }
}

impl<P: StateProvider> crate::BytecodeReader for EvmStateProviderAdapter<P> {
    fn bytecode_by_hash(&self, hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.0.bytecode_by_hash(hash)
    }

    fn legacy_code_kind(&self, hash: &B256) -> ProviderResult<Option<bool>> {
        self.0.legacy_code_kind(hash)
    }

    fn legacy_bytecode_by_hash(
        &self,
        hash: &B256,
    ) -> ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.0.legacy_bytecode_by_hash(hash)
    }

    fn legacy_delegation(
        &self,
        hash: &B256,
    ) -> reth_storage_errors::provider::ProviderResult<Option<alloy_primitives::Address>> {
        self.0.legacy_delegation(hash)
    }

    fn code_chunk_descriptor(
        &self,
        hash: &B256,
    ) -> reth_storage_errors::provider::ProviderResult<Option<crate::CodeChunkDescriptor>> {
        self.0.code_chunk_descriptor(hash)
    }

    fn get_code_chunk_by_hash(
        &self,
        hash: &B256,
        index: u32,
    ) -> ProviderResult<Option<alloy_primitives::Bytes>> {
        self.0.get_code_chunk_by_hash(hash, index)
    }

    fn get_required_code_chunk(
        &self,
        hash: &B256,
        representation: &crate::CodeRepresentation,
        index: u32,
    ) -> ProviderResult<Option<alloy_primitives::Bytes>> {
        self.0.get_required_code_chunk(hash, representation, index)
    }
}
