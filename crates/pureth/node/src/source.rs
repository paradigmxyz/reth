//! Captured source acquisition over the existing canonical provider and retained block graph.

use crate::{received_source, ExecutionEngineValidator};
use alloy_consensus::BlockHeader;
use alloy_eip7928::bal::Bal;
use alloy_eips::{BlockId, BlockNumberOrTag};
use alloy_primitives::B256;
use alloy_rlp::Decodable;
use parking_lot::{Mutex, MutexGuard};
use reth_chainspec::{ChainSpecProvider, EthereumHardforks, Hardforks};
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    BalProvider, BlockHashReader, BlockNumReader, BlockReader, ProviderError, ReceiptProvider,
    TransactionVariant,
};
use reth_pureth_block::{
    ExecutionPrimitives, ExecutionSource, ExecutionSourceProvider, PayloadSourceError,
    PayloadSourceInput, PayloadSourceKind, PayloadSourceMetadata,
};
use reth_pureth_receipt::{CanonicalityStatus, RootContext};
use schnellru::{ByLength, LruMap};
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

/// Shares retained sources and checked historical readback. The entry limit bounds cache
/// residency, not source allocation, provider I/O, active consumers or process RSS.
#[derive(Debug)]
pub struct ExecutionSourceBackend<N: ProviderNodeTypes> {
    provider: BlockchainProvider<N>,
    validator: ExecutionEngineValidator,
    sources: Mutex<LruMap<B256, Arc<SourceEntry>, ByLength>>,
}

impl<N> ExecutionSourceBackend<N>
where
    N: ProviderNodeTypes<Primitives = ExecutionPrimitives>,
    N::ChainSpec: EthereumHardforks + Hardforks,
{
    /// Uses an explicit retention policy and the same validated activation configuration as
    /// builder/receiver integration. No production retention budget is inferred here.
    pub fn new(
        provider: BlockchainProvider<N>,
        validator: ExecutionEngineValidator,
        retained_entries: NonZeroU32,
    ) -> Result<Self, PayloadSourceError> {
        validator.validate_chain_spec(provider.chain_spec().as_ref())?;
        Ok(Self {
            provider,
            validator,
            sources: Mutex::new(LruMap::new(ByLength::new(retained_entries.get()))),
        })
    }

    fn acquire(
        &self,
        selector: BlockId,
        full: bool,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        check_cancelled(cancelled)?;
        let memory = self.provider.canonical_in_memory_state();
        // Moving markers are captured once, then checked against the canonical view below.
        let pending = matches!(selector, BlockId::Number(BlockNumberOrTag::Pending))
            .then(|| memory.pending_state())
            .flatten();
        let captured_head = memory.get_canonical_head();
        let marker = match selector {
            BlockId::Number(BlockNumberOrTag::Safe) => memory.get_safe_num_hash(),
            BlockId::Number(BlockNumberOrTag::Finalized) => memory.get_finalized_num_hash(),
            _ => None,
        };
        let view = self
            .provider
            .consistent_provider()
            .map_err(|error| read_error(PayloadSourceInput::ConsistentView, error))?;
        let head = captured_head.number();
        let canonical_hash = |number| {
            if number > head {
                return Ok(None);
            }
            view.block_hash(number)
                .map_err(|error| read_error(PayloadSourceInput::CanonicalHash, error))
        };
        if canonical_hash(head)? != Some(captured_head.hash()) {
            return Err(PayloadSourceError::SourceMismatch);
        }
        let hash = match selector {
            BlockId::Hash(hash) => hash.block_hash,
            BlockId::Number(BlockNumberOrTag::Pending) => {
                let pending = pending
                    .as_ref()
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Block))?;
                if pending.number() != head.saturating_add(1) ||
                    Some(pending.block_ref().recovered_block().parent_hash()) !=
                        canonical_hash(head)?
                {
                    return Err(PayloadSourceError::SourceMismatch);
                }
                pending.hash()
            }
            BlockId::Number(BlockNumberOrTag::Safe | BlockNumberOrTag::Finalized) => {
                let marker =
                    marker.ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Block))?;
                if marker.number > head || canonical_hash(marker.number)? != Some(marker.hash) {
                    return Err(PayloadSourceError::SourceMismatch);
                }
                marker.hash
            }
            BlockId::Number(number) => {
                let number = match number {
                    BlockNumberOrTag::Number(number) => number,
                    BlockNumberOrTag::Latest => head,
                    BlockNumberOrTag::Earliest => view
                        .earliest_block_number()
                        .map_err(|error| read_error(PayloadSourceInput::Block, error))?,
                    _ => unreachable!("moving markers handled above"),
                };
                canonical_hash(number)?
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Block))?
            }
        };
        check_cancelled(cancelled)?;
        let entry = self
            .sources
            .lock()
            .get_or_insert(hash, || Arc::new(SourceEntry::default()))
            .expect("nonzero cache capacity")
            .clone();
        let executed = match pending {
            Some(state) => Some(state.block()),
            None => view
                .executed_block_by_hash(hash)
                .map_err(|error| read_error(PayloadSourceInput::Block, error))?
                .or_else(|| self.provider.retained_executed_block(hash)),
        };
        let retained = executed.as_ref().map(received_source).transpose();
        let retained = match retained {
            Ok(source) => source,
            Err(PayloadSourceError::MissingInput(_)) => None,
            Err(error) => return Err(error),
        };
        let cached = entry.receipts.try_lock().and_then(|slot| slot.clone());
        let block = if let Some(executed) = &executed {
            executed.recovered_block.clone()
        } else if let Some(source) = &cached {
            source.block_arc().clone()
        } else {
            Arc::new(
                view.recovered_block(hash.into(), TransactionVariant::WithHash)
                    .map_err(|error| read_error(PayloadSourceInput::Block, error))?
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Block))?,
            )
        };
        if block.hash() != hash {
            return Err(PayloadSourceError::SourceMismatch);
        }
        if block.transaction_count() > self.validator.limits().transactions {
            return Err(PayloadSourceError::LimitExceeded);
        }
        let identity = block.num_hash();
        let canonicality = match canonical_hash(block.number())? {
            Some(expected) if expected == hash && block.number() <= head => {
                CanonicalityStatus::Canonical
            }
            Some(_) => CanonicalityStatus::NonCanonical,
            None if block.number() > head => CanonicalityStatus::NonCanonical,
            None => CanonicalityStatus::Unknown,
        };
        if matches!(selector, BlockId::Hash(hash) if hash.require_canonical == Some(true)) &&
            canonicality != CanonicalityStatus::Canonical
        {
            return Err(PayloadSourceError::NonCanonicalBlock(identity));
        }
        if canonicality == CanonicalityStatus::Canonical &&
            block.number() > 0 &&
            canonical_hash(block.number() - 1)? != Some(block.parent_hash())
        {
            return Err(PayloadSourceError::SourceMismatch);
        }
        let cached_full = entry.payload.try_lock().and_then(|slot| slot.clone());
        if let Some(source) = retained
            .as_ref()
            .filter(|source| !full || source.payload().is_ok())
            .or(cached_full.as_ref())
            .or_else(|| cached.as_ref().filter(|_| !full))
        {
            drop(view);
            check_cancelled(cancelled)?;
            return Ok(Arc::new(source.with_canonicality(canonicality)));
        }
        let receipts = if let Some(source) = retained.as_ref().or(cached.as_ref()) {
            source.native_receipts_arc().clone()
        } else if let Some(executed) = &executed {
            Arc::new(executed.execution_outcome().result.receipts.clone())
        } else {
            Arc::new(
                view.receipts_by_block(hash.into())
                    .map_err(|error| read_error(PayloadSourceInput::Receipts, error))?
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Receipts))?,
            )
        };
        let requests = if full {
            // A live full source returned above already owns its real requests. Without that
            // attachment, require actual durable bodies: disk-to-memory execution outcomes may
            // substitute default empty requests for missing historical inputs.
            Some(
                view.execution_requests(identity)
                    .map_err(|error| read_error(PayloadSourceInput::Requests, error))?
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::Requests))?,
            )
        } else {
            None
        };
        let raw_bal = full
            .then(|| {
                executed.as_ref().and_then(|block| block.bal().map(|bal| bal.as_raw().clone()))
            })
            .flatten();
        // Copy/pin native inputs, then release the DB transaction before hashing or waiting.
        drop(view);
        let receipt_source = {
            let mut slot = lock_cancelled(&entry.receipts, cancelled)?;
            if slot.is_none() {
                let source = if let Some(source) = retained {
                    source
                } else {
                    Arc::new(ExecutionSource::from_receipts(
                        block,
                        receipts,
                        PayloadSourceMetadata {
                            native_block: identity,
                            source_kind: PayloadSourceKind::Historical,
                            canonicality: CanonicalityStatus::Unknown,
                            root_context: RootContext::RethExperimentalUnanchored,
                        },
                        self.validator.chain_spec().as_ref(),
                        &self.validator.fork_config(),
                        cancelled,
                    )?)
                };
                *slot = Some(source);
            }
            slot.as_ref().expect("initialized above").clone()
        };
        if !full {
            return Ok(Arc::new(receipt_source.with_canonicality(canonicality)));
        }
        let mut slot = lock_cancelled(&entry.payload, cancelled)?;
        if slot.is_none() {
            check_cancelled(cancelled)?;
            let raw = if let Some(raw) = raw_bal {
                raw
            } else {
                self.provider
                    .bal_store()
                    .get_by_hash(hash)
                    .map_err(|error| read_error(PayloadSourceInput::BlockAccessList, error))?
                    .ok_or(PayloadSourceError::MissingInput(PayloadSourceInput::BlockAccessList))?
            };
            if raw.len() > self.validator.limits().bal_bytes {
                return Err(PayloadSourceError::LimitExceeded);
            }
            let mut input = raw.as_ref();
            let bal = Bal::decode(&mut input).map_err(PayloadSourceError::BalDecode)?;
            if !input.is_empty() || alloy_rlp::encode(&bal).as_slice() != raw.as_ref() {
                return Err(PayloadSourceError::NonCanonicalBal);
            }
            let source = receipt_source.with_payload(
                requests.as_ref().expect("full acquisition"),
                &bal,
                self.validator.chain_spec().as_ref(),
                &self.validator.fork_config(),
                self.validator.limits(),
                cancelled,
            )?;
            *slot = Some(Arc::new(source));
        }
        check_cancelled(cancelled)?;
        Ok(Arc::new(slot.as_ref().expect("initialized above").with_canonicality(canonicality)))
    }
}

impl<N> ExecutionSourceProvider for ExecutionSourceBackend<N>
where
    N: ProviderNodeTypes<Primitives = ExecutionPrimitives>,
    N::ChainSpec: EthereumHardforks + Hardforks,
{
    fn receipt_source(
        &self,
        block: BlockId,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        self.acquire(block, false, cancelled)
    }

    fn payload_source(
        &self,
        block: BlockId,
        cancelled: &AtomicBool,
    ) -> Result<Arc<ExecutionSource>, PayloadSourceError> {
        self.acquire(block, true, cancelled)
    }
}

#[derive(Debug, Default)]
struct SourceEntry {
    receipts: Mutex<Option<Arc<ExecutionSource>>>,
    payload: Mutex<Option<Arc<ExecutionSource>>>,
}

const fn read_error(input: PayloadSourceInput, error: ProviderError) -> PayloadSourceError {
    PayloadSourceError::ProviderRead { input, error }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), PayloadSourceError> {
    if cancelled.load(Ordering::Relaxed) || reth_tasks::cancel::is_cancelled() {
        return Err(PayloadSourceError::Cancelled);
    }
    Ok(())
}

fn lock_cancelled<'a, T>(
    mutex: &'a Mutex<T>,
    cancelled: &AtomicBool,
) -> Result<MutexGuard<'a, T>, PayloadSourceError> {
    loop {
        check_cancelled(cancelled)?;
        if let Some(guard) = mutex.try_lock_for(Duration::from_millis(10)) {
            return Ok(guard);
        }
    }
}
