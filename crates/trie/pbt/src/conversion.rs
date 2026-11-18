//! Exact, hash-ordered merge of retained MPT state with EIP-8347 preimages.

use crate::{
    migration::{check_order, mpt_root, ArtifactError, DecodeLimits, Reader},
    AccountData, StateError, StateTree, TreeHasher,
};
use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, KECCAK256_EMPTY};

pub use reth_storage_api::BytecodeReader;
pub use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
pub use reth_trie::hashed_cursor::{HashedCursor, HashedCursorFactory};

/// Converts a complete, pinned MPT view into a PBT, authenticating the result against its header.
///
/// `source` must enumerate the state at the selected finalized anchor, including all storage.
/// `preimages` is the canonical EIP-8347 file for that state, not a retained preimage-store dump.
/// Missing and surplus account/slot preimages are rejected during the merge. Code is fetched
/// once per hash and checked before insertion. A wrong source view fails the final MPT check.
/// `should_stop` is checked before every account and slot; returning `true` cancels conversion.
/// This bounded reference converter keeps the resulting PBT in memory.
pub fn convert_mpt<H: TreeHasher>(
    source: &impl HashedCursorFactory,
    code_store: &impl BytecodeReader,
    preimages: &[u8],
    anchor_mpt_root: B256,
    limits: DecodeLimits,
    mut should_stop: impl FnMut() -> bool,
) -> Result<StateTree<H>, ConversionError> {
    if should_stop() {
        return Err(ConversionError::Cancelled);
    }
    let mut reader = Reader(preimages);
    let mut accounts = source.hashed_account_cursor()?;
    let mut next_account = accounts.seek(B256::ZERO)?;
    let mut state = StateTree::<H>::new();
    let mut codes = B256Map::default();
    let mut total_code_size = 0usize;
    let mut previous_account = None;
    let mut account_count = 0usize;
    while !reader.0.is_empty() {
        if should_stop() {
            return Err(ConversionError::Cancelled);
        }
        account_count += 1;
        if account_count > limits.max_accounts {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let address = Address::from_slice(reader.take(20)?);
        let hash = keccak256(address);
        check_order(&mut previous_account, hash)?;
        let Some((source_hash, account)) = next_account.take() else {
            return Err(ConversionError::SurplusAccount(address));
        };
        if source_hash < hash {
            return Err(ConversionError::MissingAccount(source_hash));
        }
        if source_hash > hash {
            return Err(ConversionError::SurplusAccount(address));
        }
        let count = u32::from_be_bytes(reader.array()?) as usize;
        if count > limits.max_leaves || count > reader.0.len() / 32 {
            return Err(ArtifactError::ResourceLimit.into());
        }
        let code_hash = account.bytecode_hash.unwrap_or(KECCAK256_EMPTY);
        let code = if code_hash == KECCAK256_EMPTY {
            Bytes::new()
        } else if let Some(code) = codes.get(&code_hash) {
            Bytes::clone(code)
        } else {
            let bytecode = code_store
                .bytecode_by_hash(&code_hash)?
                .ok_or(ConversionError::MissingCode(code_hash))?;
            let code = bytecode.original_bytes();
            // Delegations live in account headers, outside the contract-code chunk budget.
            if !bytecode.is_eip7702() {
                if code.len() > limits.max_code_size {
                    return Err(ArtifactError::ResourceLimit.into());
                }
                total_code_size =
                    total_code_size.checked_add(code.len()).ok_or(ArtifactError::ResourceLimit)?;
                if total_code_size > limits.max_total_code_size {
                    return Err(ArtifactError::ResourceLimit.into());
                }
            }
            if keccak256(&code) != code_hash {
                return Err(ConversionError::MissingCode(code_hash));
            }
            codes.insert(code_hash, code.clone());
            code
        };
        let data = AccountData { nonce: account.nonce, balance: account.balance, code };
        if data.is_empty() {
            return Err(ArtifactError::Invalid("empty anchor account").into());
        }
        state.set_account(address, data)?;
        let mut storage = source.hashed_storage_cursor(hash)?;
        let mut next_slot = storage.seek(B256::ZERO)?;
        let mut previous_slot = None;
        for _ in 0..count {
            if should_stop() {
                return Err(ConversionError::Cancelled);
            }
            let slot = B256::from_slice(reader.take(32)?);
            let slot_hash = keccak256(slot);
            check_order(&mut previous_slot, slot_hash)?;
            let Some((source_slot, value)) = next_slot.take() else {
                return Err(ConversionError::SurplusSlot { address, slot });
            };
            if source_slot < slot_hash {
                return Err(ConversionError::MissingSlot { address, slot_hash: source_slot });
            }
            if source_slot > slot_hash {
                return Err(ConversionError::SurplusSlot { address, slot });
            }
            if value.is_zero() {
                return Err(ArtifactError::Invalid("zero-valued MPT storage leaf").into());
            }
            state.set_storage(address, slot.into(), B256::from(value))?;
            if state.tree().len() > limits.max_leaves {
                return Err(ArtifactError::ResourceLimit.into());
            }
            next_slot = storage.next()?;
        }
        if let Some((slot_hash, _)) = next_slot {
            return Err(ConversionError::MissingSlot { address, slot_hash });
        }
        if state.tree().len() > limits.max_leaves {
            return Err(ArtifactError::ResourceLimit.into());
        }
        next_account = accounts.next()?;
    }
    if let Some((hash, _)) = next_account {
        return Err(ConversionError::MissingAccount(hash));
    }
    if mpt_root(&state) != anchor_mpt_root {
        return Err(ArtifactError::AnchorRootMismatch.into());
    }
    Ok(state)
}

/// Conversion failed before an anchor checkpoint could be published.
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    /// The caller requested cancellation before the next account or slot was applied.
    #[error("MPT conversion cancelled")]
    Cancelled,
    /// A source account has no matching raw address.
    #[error("missing account preimage for {0}")]
    MissingAccount(B256),
    /// A preimage names an account absent from the pinned source.
    #[error("surplus account preimage for {0}")]
    SurplusAccount(Address),
    /// A source storage leaf has no matching plain slot key.
    #[error("missing storage preimage {slot_hash} for {address}")]
    MissingSlot {
        /// Account owning the storage trie.
        address: Address,
        /// Source MPT path that could not be resolved.
        slot_hash: B256,
    },
    /// A preimage names a slot absent from the pinned source.
    #[error("surplus storage preimage {slot} for {address}")]
    SurplusSlot {
        /// Account named by the preimage record.
        address: Address,
        /// Plain storage slot absent from its source trie.
        slot: B256,
    },
    /// Referenced bytecode is missing or does not match its Keccak hash.
    #[error("missing or inconsistent source bytecode for {0}")]
    MissingCode(B256),
    /// Reading a source hashed-state cursor failed.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// Reading source bytecode failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The preimage file, resource budget, or anchor commitment is invalid.
    #[error(transparent)]
    Artifact(#[from] ArtifactError),
    /// A source account cannot be represented in the PBT.
    #[error(transparent)]
    State(#[from] StateError),
}
