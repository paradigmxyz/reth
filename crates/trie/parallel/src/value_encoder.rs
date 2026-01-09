use crate::proof_task::{
    StorageProofInput, StorageProofResult, StorageProofResultMessage, StorageWorkerJob,
};
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use core::cell::RefCell;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use dashmap::DashMap;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie::{
    proof_v2::{DeferredValueEncoder, LeafValueEncoder, Target},
    ProofTrieNode,
};
use std::{cell::Cell, rc::Rc, sync::Arc};

/// Tracks the receiver used for receiving the results from storage proof jobs, as well as a Cell
/// which is used to exfiltrate those results after they've been used within the
/// [`AsyncAccountDeferredValueEncoder`].
pub(crate) struct DispatchedStorageProof {
    proof_result_rx: CrossbeamReceiver<StorageProofResultMessage>,
    proof_nodes: Cell<Option<Result<StorageProofResult, StateProofError>>>,
}

/// Returned from [`AsyncAccountValueEncoder`], used to track an async storage root calculation.
pub(crate) enum AsyncAccountDeferredValueEncoder {
    Dispatched {
        hashed_address: B256,
        account: Account,
        proof_result_rx: Result<CrossbeamReceiver<StorageProofResultMessage>, DatabaseError>,
        storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>,
    },
    FromCache {
        account: Account,
        root: B256,
    },
}

impl DeferredValueEncoder for AsyncAccountDeferredValueEncoder {
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let (account, root) = match self {
            Self::Dispatched {
                hashed_address,
                account,
                proof_result_rx,
                storage_proof_results,
            } => {
                let result = proof_result_rx?
                    .recv()
                    .map_err(|_| {
                        StateProofError::Database(DatabaseError::Other(format!(
                            "Storage proof channel closed for {hashed_address:?}",
                        )))
                    })?
                    .result?;

                let StorageProofResult::V2 { root: Some(root), proof } = result else {
                    panic!("StorageProofResult is not V2 with root: {result:?}")
                };

                storage_proof_results.borrow_mut().insert(hashed_address, proof);

                (account, root)
            }
            Self::FromCache { account, root } => (account, root),
        };

        let account = account.into_trie_account(root);
        account.encode(buf);
        Ok(())
    }
}

/// Implements the [`LeafValueEncoder`] trait for accounts using a [`CrossbeamSender`] to dispatch
/// and compute storage roots asyncronously. Can also accept a set of already dispatched account
/// storage proofs, for cases where it's possible to determine some necessary accounts ahead of
/// time.
pub(crate) struct AsyncAccountValueEncoder {
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    /// Storage proof jobs which were dispatched ahead of time.
    dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    /// Storage roots which have already been computed. This can be used only if a storage proof
    /// wasn't dispatched for an account, otherwise we must consume the proof result.
    cached_storage_roots: Arc<DashMap<B256, B256>>,
    /// Tracks storage proof results received from the storage workers. [`Rc`] + [`RefCell`] is
    /// required because [`DeferredValueEncoder`] cannot have a lifetime.
    storage_proof_results: Rc<RefCell<B256Map<Vec<ProofTrieNode>>>>,
}

impl AsyncAccountValueEncoder {
    /// Initializes a [`Self`] using a [`ProofWorkerHandle`] which will be used to calculate storage
    /// roots asynchronously.
    pub(crate) fn new(
        storage_work_tx: CrossbeamSender<StorageWorkerJob>,
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
        cached_storage_roots: Arc<DashMap<B256, B256>>,
    ) -> Self {
        Self {
            storage_work_tx,
            dispatched,
            cached_storage_roots,
            storage_proof_results: Default::default(),
        }
    }

    ///// Consume [`Self`] and return all collected storage proofs which had been dispatched.
    //pub(crate) fn into_storage_proofs(
    //    mut self,
    //) -> Result<B256Map<Vec<ProofTrieNode>>, DatabaseError> {
    //    let mut storage_proof_results = self.storage_proof_results.into_inner();
    //}
}

impl LeafValueEncoder for AsyncAccountValueEncoder {
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder;

    fn deferred_encoder(
        &mut self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // If the proof job has already been dispatched for this account then it's not necessary to
        // dispatch another.
        if let Some(rx) = self.dispatched.get(&hashed_address) {
            return AsyncAccountDeferredValueEncoder::Dispatched {
                hashed_address,
                account,
                proof_result_rx: Ok(rx.clone()),
                storage_proof_results: self.storage_proof_results.clone(),
            }
        }

        // If the address didn't have a job dispatched for it then we can assume it has no targets,
        // and we only need its root.

        // If the root is already calculated then just use it directly
        if let Some(root) = self.cached_storage_roots.get(&hashed_address) {
            return AsyncAccountDeferredValueEncoder::FromCache { account, root: *root }
        }

        // Create a proof input which targets a bogus key, so that we calculate the root as a
        // side-effect.
        let input = StorageProofInput::new(hashed_address, vec![Target::new(B256::ZERO)]);
        let (tx, rx) = crossbeam_channel::bounded(1);

        let proof_result_rx = self
            .storage_work_tx
            .send(StorageWorkerJob::StorageProof { input, proof_result_sender: tx })
            .map_err(|_| DatabaseError::Other("storage workers unavailable".to_string()))
            .map(|_| rx);

        AsyncAccountDeferredValueEncoder::Dispatched {
            hashed_address,
            account,
            proof_result_rx,
            storage_proof_results: self.storage_proof_results.clone(),
        }
    }
}
