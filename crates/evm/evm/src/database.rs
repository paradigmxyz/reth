//! State provider database adapter used by EVM execution.

#[cfg(feature = "std")]
use alloy_primitives::{Address, B256};
use core::ops::{Deref, DerefMut};
#[cfg(feature = "std")]
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
    interpreter::Word,
};
#[cfg(feature = "std")]
use reth_storage_api::EvmStateProvider;
#[cfg(feature = "std")]
use reth_storage_errors::provider::ProviderError;

/// A database wrapper backed by a [`reth_storage_api::EvmStateProvider`].
#[derive(Clone)]
pub struct StateProviderDatabase<DB>(pub DB);

impl<DB> StateProviderDatabase<DB> {
    /// Creates a new database wrapper with the given state provider.
    pub const fn new(db: DB) -> Self {
        Self(db)
    }

    /// Consumes the wrapper and returns the inner state provider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB> core::fmt::Debug for StateProviderDatabase<DB> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StateProviderDatabase").finish_non_exhaustive()
    }
}

impl<DB> AsRef<DB> for StateProviderDatabase<DB> {
    fn as_ref(&self) -> &DB {
        &self.0
    }
}

impl<DB> Deref for StateProviderDatabase<DB> {
    type Target = DB;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<DB> DerefMut for StateProviderDatabase<DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(feature = "std")]
impl<DB> Database for StateProviderDatabase<DB>
where
    DB: EvmStateProvider,
{
    type Error = ProviderError;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let Some(account) = self.0.basic_account(address)? else { return Ok(None) };
        let native = reth_execution_types::native_provider_account(&account)
            .map_err(ProviderError::other)?;
        if let Some(metadata) = &native.code_metadata {
            let stored = self.0.code_chunk_descriptor(&native.code_hash)?.ok_or_else(|| {
                reth_storage_errors::provider::CodeChunkError {
                    code_hash: native.code_hash,
                    index: 0,
                    expected_length: metadata.chunk_len(0),
                    reason: reth_storage_errors::provider::CodeChunkErrorKind::MissingDescriptor,
                    context: None,
                }
            })?;
            if stored.code_size() != metadata.code_size() ||
                stored.chunk_hashes() != metadata.chunk_hashes()
            {
                return Err(reth_storage_errors::provider::CodeChunkError {
                    code_hash: native.code_hash,
                    index: 0,
                    expected_length: metadata.chunk_len(0),
                    reason: reth_storage_errors::provider::CodeChunkErrorKind::DescriptorMismatch,
                    context: None,
                }
                .into())
            }
        }
        Ok(Some(native))
    }

    fn get_code_kind_by_hash(
        &mut self,
        hash: &B256,
    ) -> Result<evm2::bytecode::BytecodeKind, Self::Error> {
        if *hash == alloy_primitives::KECCAK256_EMPTY {
            return Ok(evm2::bytecode::BytecodeKind::Legacy)
        }
        if let Some(delegated) = self.0.legacy_code_kind(hash)? {
            return Ok(if delegated {
                evm2::bytecode::BytecodeKind::Eip7702
            } else {
                evm2::bytecode::BytecodeKind::Legacy
            })
        }
        if self.0.code_chunk_descriptor(hash)?.is_some() {
            return Ok(evm2::bytecode::BytecodeKind::Legacy)
        }
        Err(reth_storage_errors::provider::CodeChunkError {
            code_hash: *hash,
            index: 0,
            expected_length: None,
            reason: reth_storage_errors::provider::CodeChunkErrorKind::MissingCode,
            context: None,
        }
        .into())
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        if *code_hash == alloy_primitives::KECCAK256_EMPTY {
            return Ok(Bytecode::default())
        }
        match self.0.legacy_bytecode_by_hash(code_hash)? {
            Some(code) => Some(code),
            None => self.0.bytecode_by_hash(code_hash)?,
        }
        .map(|code| reth_execution_types::native_bytecode(&code.0))
        .ok_or_else(|| {
            reth_storage_errors::provider::CodeChunkError {
                code_hash: *code_hash,
                index: 0,
                expected_length: None,
                reason: reth_storage_errors::provider::CodeChunkErrorKind::MissingCode,
                context: None,
            }
            .into()
        })
    }

    fn get_code_chunk_by_hash(
        &mut self,
        hash: &B256,
        index: u32,
    ) -> Result<Option<evm2::bytecode::CodeChunk>, Self::Error> {
        if let Some(descriptor) = self.0.code_chunk_descriptor(hash)? {
            let Some(prepared) = descriptor.preparation(index) else { return Ok(None) };
            let bytes = self.0.get_required_code_chunk(
                hash,
                &reth_storage_api::CodeRepresentation::Chunked(descriptor.clone()),
                index,
            )?;
            return bytes
                .map(|bytes| {
                    evm2::bytecode::CodeChunk::with_preparation(
                        bytes,
                        descriptor.code_size(),
                        index,
                        prepared.leading_data_len,
                        prepared.jump_data_len,
                        prepared.lookahead.clone(),
                    )
                    .map_err(ProviderError::other)
                })
                .transpose();
        }
        if index != 0 {
            return Ok(None);
        }
        let Some(bytes) = self.0.get_code_chunk_by_hash(hash, index)? else { return Ok(None) };
        let code = if let Some(target) = self.0.legacy_delegation(hash)? {
            Bytecode::new_eip7702(target)
        } else {
            Bytecode::new_legacy(bytes)
        };
        Ok(Some(evm2::bytecode::CodeChunk::from_bytecode(&code)))
    }

    fn get_storage(&mut self, address: &Address, key: &Word) -> Result<Word, Self::Error> {
        Ok(self.0.storage(*address, B256::new(key.to_be_bytes()))?.unwrap_or_default())
    }

    fn get_block_hash(&mut self, number: &Word) -> Result<B256, Self::Error> {
        let number = number.saturating_to::<u64>();
        Ok(self.0.block_hash(number)?.unwrap_or_default())
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use reth_storage_api::{noop::NoopProvider, StateProvider};

    #[test]
    fn missing_block_hash_matches_revm_adapter() {
        let mut native =
            StateProviderDatabase::new(NoopProvider::mainnet().into_evm_state_provider());
        let mut revm = reth_revm::database::StateProviderDatabase::new(
            NoopProvider::mainnet().into_evm_state_provider(),
        );
        assert_eq!(
            native.get_block_hash(&Word::from(42)).unwrap(),
            revm::Database::block_hash(&mut revm, 42).unwrap()
        );
    }
}

#[cfg(feature = "std")]
impl<DB: reth_storage_api::EvmStateProvider> reth_storage_api::BytecodeReader
    for StateProviderDatabase<DB>
{
    fn bytecode_by_hash(
        &self,
        hash: &alloy_primitives::B256,
    ) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives_traits::Bytecode>>
    {
        self.0.bytecode_by_hash(hash)
    }

    fn legacy_code_kind(
        &self,
        hash: &B256,
    ) -> reth_storage_errors::provider::ProviderResult<Option<bool>> {
        self.0.legacy_code_kind(hash)
    }

    fn legacy_bytecode_by_hash(
        &self,
        hash: &B256,
    ) -> reth_storage_errors::provider::ProviderResult<Option<reth_primitives_traits::Bytecode>>
    {
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
    ) -> reth_storage_errors::provider::ProviderResult<Option<reth_storage_api::CodeChunkDescriptor>>
    {
        self.0.code_chunk_descriptor(hash)
    }

    fn get_code_chunk_by_hash(
        &self,
        hash: &alloy_primitives::B256,
        index: u32,
    ) -> reth_storage_errors::provider::ProviderResult<Option<alloy_primitives::Bytes>> {
        self.0.get_code_chunk_by_hash(hash, index)
    }

    fn get_required_code_chunk(
        &self,
        hash: &alloy_primitives::B256,
        representation: &reth_storage_api::CodeRepresentation,
        index: u32,
    ) -> reth_storage_errors::provider::ProviderResult<Option<alloy_primitives::Bytes>> {
        self.0.get_required_code_chunk(hash, representation, index)
    }
}
