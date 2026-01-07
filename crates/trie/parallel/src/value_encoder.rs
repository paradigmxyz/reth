use crate::proof_task::{
    ProofWorkerHandle, StorageProofInput, StorageProofResult, StorageProofResultMessage,
};
use alloy_primitives::{map::B256Map, B256};
use alloy_rlp::Encodable;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_execution_errors::trie::StateProofError;
use reth_primitives_traits::Account;
use reth_provider::ProviderError;
use reth_storage_errors::db::DatabaseError;
use reth_trie::proof_v2::{DeferredValueEncoder, LeafValueEncoder, Target};

/// Returned from [`AsyncAccountValueEncoder`], used to track an async storage root calculation.
pub(crate) struct AsyncAccountDeferredValueEncoder {
    hashed_address: B256,
    account: Account,
    proof_result_rx: Result<CrossbeamReceiver<StorageProofResultMessage>, ProviderError>,
}

impl DeferredValueEncoder for AsyncAccountDeferredValueEncoder {
    fn encode(self, buf: &mut Vec<u8>) -> Result<(), StateProofError> {
        let result = self
            .proof_result_rx
            .map_err(|err| {
                StateProofError::Database(DatabaseError::Other(format!(
                    "Failed to dispatch storage proof for {}: {err}",
                    self.hashed_address,
                )))
            })?
            .recv()
            .map_err(|_| {
                StateProofError::Database(DatabaseError::Other(format!(
                    "Storage proof channel closed for {}",
                    self.hashed_address,
                )))
            })?
            .result?;

        let StorageProofResult::V2 { root: Some(root), .. } = result else {
            panic!("StorageProofResult is not V2 with root: {result:?}")
        };

        let account = self.account.into_trie_account(root);
        account.encode(buf);
        Ok(())
    }
}

/// Implements the [`LeafValueEncoder`] trait for accounts using a [`ProofWorkerHandle`] to dispatch
/// and compute storage roots asyncronously. Can also accept a set of already dispatched account
/// storage roots, for cases where it's possible to determine some necessary accounts ahead of time.
pub(crate) struct AsyncAccountValueEncoder {
    handle: ProofWorkerHandle,
    dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
}

impl AsyncAccountValueEncoder {
    /// Initializes a [`Self`] using a [`ProofWorkerHandle`] which will be used to calculate storage
    /// roots asynchronously.
    pub(crate) fn new(handle: ProofWorkerHandle) -> Self {
        Self { handle, dispatched: B256Map::default() }
    }

    /// Use the given set of already dispatched storage root calculations.
    pub(crate) fn with_dispatched(
        mut self,
        dispatched: B256Map<CrossbeamReceiver<StorageProofResultMessage>>,
    ) -> Self {
        self.dispatched = dispatched;
        self
    }
}

impl LeafValueEncoder for AsyncAccountValueEncoder {
    type Value = Account;
    type DeferredEncoder = AsyncAccountDeferredValueEncoder;

    fn deferred_encoder(
        &self,
        hashed_address: B256,
        account: Self::Value,
    ) -> Self::DeferredEncoder {
        // If the proof job has already been dispatched for this account then it's not necessary to
        // dispatch another.
        if let Some(rx) = self.dispatched.get(&hashed_address) {
            return AsyncAccountDeferredValueEncoder {
                hashed_address,
                account,
                proof_result_rx: Ok(rx.clone()),
            }
        }

        // Create a proof input which just targets a bogus key, so that we calculate the root as a
        // side-effect.
        let input = StorageProofInput::new(hashed_address, vec![Target::new(B256::ZERO)]);
        let (tx, rx) = crossbeam_channel::bounded(1);

        AsyncAccountDeferredValueEncoder {
            hashed_address,
            account,
            proof_result_rx: self.handle.dispatch_storage_proof(input, tx).map(|_| rx),
        }
    }
}
