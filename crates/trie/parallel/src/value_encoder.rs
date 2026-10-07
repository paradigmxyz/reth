//! Deferred account encoding and collection of requested storage proofs.

use crate::storage_proof::StorageProofResultMessage;
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_execution_errors::StateProofError;
use reth_primitives_traits::Account;
use reth_trie::{
    hashed_cursor::HashedStorageCursor,
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, StorageProofCalculator},
    trie_cursor::TrieStorageCursor,
    ProofTrieNodeV2,
};
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

/// Account encoder sharing pending storage work with its deferred values.
pub(crate) struct AsyncAccountValueEncoder<TC, HC> {
    storage: Rc<RefCell<StorageProofs>>,
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
}

impl<TC, HC> AsyncAccountValueEncoder<TC, HC> {
    pub(crate) fn new(
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
        storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
    ) -> Self {
        Self {
            storage: Rc::new(RefCell::new(StorageProofs {
                pending: dispatched,
                results: Default::default(),
                stats: Default::default(),
            })),
            storage_calculator,
        }
    }

    /// Collects every requested storage proof, including work returned by dropped encoders.
    ///
    /// # Panics
    ///
    /// Panics if any deferred encoders remain alive.
    pub(crate) fn finalize(
        self,
    ) -> Result<(B256Map<Vec<ProofTrieNodeV2>>, ValueEncoderStats), StateProofError> {
        let mut storage = Rc::into_inner(self.storage)
            .expect("no deferred encoders are still allocated")
            .into_inner();
        for (address, rx) in core::mem::take(&mut storage.pending) {
            storage.collect(address, rx)?;
        }
        Ok((storage.results, storage.stats))
    }
}

impl<TC, HC> LeafValueEncoder for AsyncAccountValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder<TC, HC>;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Account,
    ) -> Self::DeferredEncoder {
        let mut storage = self.storage.borrow_mut();
        let receiver = storage.pending.remove(&hashed_address);
        if receiver.is_some() {
            storage.stats.dispatched_count += 1;
        } else {
            storage.stats.sync_count += 1;
        }
        AsyncAccountDeferredValueEncoder {
            hashed_address,
            account,
            receiver,
            storage: self.storage.clone(),
            storage_calculator: self.storage_calculator.clone(),
        }
    }
}

/// Deferred value whose unconsumed receiver is returned to finalization on drop.
pub(crate) struct AsyncAccountDeferredValueEncoder<TC, HC> {
    hashed_address: B256,
    account: Account,
    receiver: Option<CrossbeamReceiver<StorageProofResultMessage>>,
    storage: Rc<RefCell<StorageProofs>>,
    storage_calculator: Rc<RefCell<StorageProofCalculator<TC, HC>>>,
}

impl<TC, HC> Drop for AsyncAccountDeferredValueEncoder<TC, HC> {
    fn drop(&mut self) {
        if let Some(rx) = self.receiver.take() {
            self.storage.borrow_mut().pending.insert(self.hashed_address, rx);
        }
    }
}

impl<TC, HC> DeferredValueEncoder for AsyncAccountDeferredValueEncoder<TC, HC>
where
    TC: TrieStorageCursor,
    HC: HashedStorageCursor<Value = alloy_primitives::U256>,
{
    #[allow(clippy::clone_on_copy)]
    fn encode(mut self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let dispatched = self.receiver.is_some();
        let root = self
            .receiver
            .take()
            .map(|rx| self.storage.borrow_mut().collect(self.hashed_address, rx))
            .transpose()?
            .flatten();
        let root = if let Some(root) = root {
            root
        } else {
            if dispatched {
                self.storage.borrow_mut().stats.dispatched_missing_root_count += 1;
            }
            // A storage-only target may omit its root even when account traversal needs it.
            let mut calculator = self.storage_calculator.borrow_mut();
            let root_node = calculator.storage_root_node(self.hashed_address)?;
            calculator
                .compute_root_hash(&[root_node])?
                .expect("storage_root_node returns a node at empty path")
        };
        self.account.clone().into_trie_account(root).encode(buf);
        Ok(())
    }
}

/// Stats collected by [`AsyncAccountValueEncoder`] during proof computation.
///
/// Tracks time spent waiting for storage proofs and counts of each deferred encoder variant used.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ValueEncoderStats {
    /// Accumulated time spent waiting for storage proof results from dispatched workers.
    pub(crate) storage_wait_time: Duration,
    /// Number of times the `Dispatched` variant was used (proof pre-dispatched to workers).
    pub(crate) dispatched_count: u64,
    /// Number of times the `Sync` variant was used (synchronous computation).
    pub(crate) sync_count: u64,
    /// Number of times a dispatched storage proof had no root node and fell back to sync
    /// computation.
    pub(crate) dispatched_missing_root_count: u64,
}

impl ValueEncoderStats {
    /// Extends this metrics by adding the values from another.
    pub(crate) fn extend(&mut self, other: &Self) {
        self.storage_wait_time += other.storage_wait_time;
        self.dispatched_count += other.dispatched_count;
        self.sync_count += other.sync_count;
        self.dispatched_missing_root_count += other.dispatched_missing_root_count;
    }
}

#[derive(Debug)]
struct StorageProofs {
    pending: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    results: B256Map<Vec<ProofTrieNodeV2>>,
    stats: ValueEncoderStats,
}

impl StorageProofs {
    fn collect(
        &mut self,
        address: B256,
        rx: CrossbeamReceiver<StorageProofResultMessage>,
    ) -> Result<Option<B256>, StateProofError> {
        let start = Instant::now();
        let message = rx.recv();
        self.stats.storage_wait_time += start.elapsed();
        let result =
            message.map_err(|_| StateProofError::StorageProofChannelClosed(address))?.result?;
        self.results.insert(address, result.proof);
        Ok(result.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_proof::StorageProofResult;
    use reth_trie::{
        hashed_cursor::noop::NoopHashedCursor, trie_cursor::noop::NoopStorageTrieCursor,
    };

    #[test]
    fn dropped_encoder_returns_required_proof_to_finalization() {
        for outcome in 0..3 {
            let address = B256::with_last_byte(1);
            let (tx, rx) = crossbeam_channel::bounded(1);
            let calculator = StorageProofCalculator::new_storage(
                NoopStorageTrieCursor::default(),
                NoopHashedCursor::default(),
            );
            let mut encoder = AsyncAccountValueEncoder::new(
                B256Map::from_iter([(address, rx)]),
                Rc::new(RefCell::new(calculator)),
            );
            let deferred = encoder.deferred_encoder(address, Account::default());
            // Drop before the worker responds: dropping must neither block nor discard its result.
            drop(deferred);
            if outcome != 2 {
                let result = if outcome == 0 {
                    Ok(StorageProofResult { proof: vec![ProofTrieNodeV2::empty()], root: None })
                } else {
                    Err(StateProofError::TrieInconsistency("storage proof failed".into()))
                };
                tx.send(StorageProofResultMessage { hashed_address: address, result }).unwrap();
            }
            drop(tx);
            let result = encoder.finalize();
            match outcome {
                0 => assert_eq!(result.unwrap().0[&address], vec![ProofTrieNodeV2::empty()]),
                1 => assert!(
                    matches!(result, Err(StateProofError::TrieInconsistency(message)) if message == "storage proof failed")
                ),
                _ => assert!(
                    matches!(result, Err(StateProofError::StorageProofChannelClosed(a)) if a == address)
                ),
            }
        }
    }
}
