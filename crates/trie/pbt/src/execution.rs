//! Applies finalized block output using plain address/slot preimages, never MPT hashed keys.

use crate::{AccountData, StateError, StateTree, StateUpdate, TreeHasher};
use alloy_primitives::{keccak256, Bytes, B256, KECCAK256_EMPTY};
use reth_trie_common::bundle_state::BundleState;

#[cfg(feature = "persistence")]
use crate::migration::DecodeLimits;

#[cfg(feature = "persistence")]
use pbt::persistence::{AccountUpdate, CodeUpdate, DiskCheckpoint, OverlayError, StateOverlay};

/// Applies one block's execution output atomically to a parent PBT state.
///
/// The parent must contain all accounts and code. New code is obtained from the output's
/// contract table; missing or inconsistent code fails the block without changing the parent.
/// The finalized output is authoritative on serial and parallel execution paths alike.
pub fn apply_bundle<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
) -> Result<StateTree<H>, ExecutionError> {
    apply_bundle_with(parent, bundle, StateTree::apply_batch)
}

/// Applies block output with parallel hashing of dense PBT update batches.
/// Validation and parent-snapshot guarantees match [`apply_bundle`].
#[cfg(feature = "parallel")]
pub fn apply_bundle_parallel<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
) -> Result<StateTree<H>, ExecutionError> {
    apply_bundle_with(parent, bundle, StateTree::apply_batch_parallel)
}

fn apply_bundle_with<H: TreeHasher>(
    parent: &StateTree<H>,
    bundle: &BundleState,
    apply: impl FnOnce(&mut StateTree<H>, Vec<StateUpdate>) -> Result<(), StateError>,
) -> Result<StateTree<H>, ExecutionError> {
    let mut state = parent.clone();
    let mut updates = Vec::new();
    for (address, account) in bundle.state() {
        if account.status.is_not_modified() {
            continue;
        }
        let Some(info) = &account.info else {
            updates.push(StateUpdate::Account { address: *address, account: None });
            continue;
        };
        let code = match supplied_code(
            info.code_hash,
            info.code
                .as_ref()
                .or_else(|| bundle.contracts.get(&info.code_hash))
                .map(|code| || code.original_bytes()),
        )? {
            Some(bytes) => bytes,
            None => parent
                .account(*address)
                .filter(|account| keccak256(&account.code) == info.code_hash)
                .map(|account| account.code.clone())
                .ok_or(ExecutionError::MissingCode(info.code_hash))?,
        };
        if account.was_destroyed() {
            updates.push(StateUpdate::ClearStorage { address: *address });
        }
        let data = AccountData { nonce: info.nonce, balance: info.balance, code };
        let has_account = !data.is_empty();
        updates.push(StateUpdate::Account { address: *address, account: Some(data) });
        if has_account {
            for (slot, value) in &account.storage {
                updates.push(StateUpdate::Storage {
                    address: *address,
                    slot: *slot,
                    value: B256::from(value.present_value),
                });
            }
        }
    }
    apply(&mut state, updates)?;
    Ok(state)
}

/// A prepared parent for an execution state-root job.
#[cfg(feature = "persistence")]
#[derive(Debug)]
pub enum ExecutionParent<H> {
    /// Already materialized, or required for a compatibility or authentication boundary.
    Eager(StateTree<H>),
    /// A pinned checkpoint whose state is read as needed.
    Disk(DiskCheckpoint<H>),
}

#[cfg(feature = "persistence")]
impl<H: TreeHasher> ExecutionParent<H> {
    /// Applies authoritative output to an isolated child. Global code ownership is only
    /// needed when a previously committed contract is replaced or removed.
    /// Disk overlays apply serially. The parallel flag selects the eager batch implementation.
    pub fn apply_bundle(
        &self,
        bundle: &BundleState,
        parallel: bool,
    ) -> Result<ExecutionState<H>, ExecutionError> {
        let eager = |state: &StateTree<H>| {
            #[cfg(feature = "parallel")]
            if parallel {
                return apply_bundle_parallel(state, bundle).map(ExecutionState::Eager)
            }
            let _ = parallel;
            apply_bundle(state, bundle).map(ExecutionState::Eager)
        };
        match self {
            Self::Eager(state) => eager(state),
            Self::Disk(parent) => {
                let mut overlay = parent.overlay();
                match apply_to_overlay(&mut overlay, bundle) {
                    Ok(()) => Ok(ExecutionState::Disk(overlay)),
                    Err(ExecutionError::Overlay(OverlayError::NeedsFullState)) => {
                        let state =
                            parent.restore(DecodeLimits::native()).map_err(OverlayError::from)?;
                        eager(&state)
                    }
                    Err(ExecutionError::Overlay(OverlayError::MissingCode(hash))) => {
                        Err(ExecutionError::MissingCode(hash))
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
}

/// The uncommitted PBT state resulting from a block's execution.
#[cfg(feature = "persistence")]
#[derive(Debug)]
pub enum ExecutionState<H> {
    /// A complete snapshot.
    Eager(StateTree<H>),
    /// Changes over a pinned native parent.
    Disk(StateOverlay<H>),
}

#[cfg(feature = "persistence")]
impl<H: TreeHasher> ExecutionState<H> {
    /// Returns the computed PBT commitment.
    pub fn root(&self) -> B256 {
        match self {
            Self::Eager(state) => state.root(),
            Self::Disk(state) => state.root(),
        }
    }

    /// Returns whether this result used the parallel batch implementation.
    ///
    /// The eager batch implementation can use incremental hashing for sparse updates.
    pub const fn uses_parallel_batch(&self, requested: bool) -> bool {
        requested && cfg!(feature = "parallel") && matches!(self, Self::Eager(_))
    }

    /// Returns the resulting number of leaves.
    pub const fn leaf_count(&self) -> usize {
        match self {
            Self::Eager(state) => state.tree().len(),
            Self::Disk(state) => state.leaf_count(),
        }
    }
}

#[cfg(feature = "persistence")]
fn apply_to_overlay<H: TreeHasher>(
    overlay: &mut StateOverlay<H>,
    bundle: &BundleState,
) -> Result<(), ExecutionError> {
    for (address, account) in bundle.state() {
        if account.status.is_not_modified() {
            continue
        }
        let update = if let Some(info) = &account.info {
            let bytes = supplied_code(
                info.code_hash,
                info.code
                    .as_ref()
                    .or_else(|| bundle.contracts.get(&info.code_hash))
                    .map(|code| || code.original_bytes()),
            )?;
            Some(AccountUpdate {
                nonce: info.nonce,
                balance: info.balance,
                code: bytes.map_or(CodeUpdate::Retain(info.code_hash), CodeUpdate::Bytes),
            })
        } else {
            None
        };
        overlay.update_account(
            *address,
            update,
            account.was_destroyed(),
            account.storage.iter().map(|(slot, value)| (*slot, B256::from(value.present_value))),
        )?;
    }
    Ok(())
}

fn supplied_code(
    hash: B256,
    code: Option<impl FnOnce() -> Bytes>,
) -> Result<Option<Bytes>, ExecutionError> {
    if hash == KECCAK256_EMPTY || hash.is_zero() {
        if let Some(code) = code &&
            !code().is_empty()
        {
            return Err(ExecutionError::MissingCode(hash))
        }
        return Ok(Some(Bytes::new()))
    }
    if let Some(code) = code {
        let bytes = code();
        if keccak256(&bytes) != hash {
            return Err(ExecutionError::MissingCode(hash))
        }
        return Ok(Some(bytes))
    }
    Ok(None)
}

/// Invalid or incomplete execution output.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    /// The block refers to bytecode unavailable in the parent and output, or with a wrong hash.
    #[error("missing or inconsistent bytecode for {0}")]
    MissingCode(B256),
    /// Invalid Ethereum state encoding.
    #[error(transparent)]
    State(#[from] StateError),
    /// Resolving or updating a native checkpoint failed.
    #[cfg(feature = "persistence")]
    #[error(transparent)]
    Overlay(#[from] OverlayError),
}
