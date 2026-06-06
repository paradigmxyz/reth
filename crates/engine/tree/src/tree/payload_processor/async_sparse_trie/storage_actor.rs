use super::{
    drain_update_chunk, insert_leaf_update, merge_leaf_updates, proof_service::ProofServiceCommand,
    to_parallel_sparse_error, ActorTrie, CoordinatorEvent, MAX_STORAGE_UPDATES_PER_DRIVE,
};
use alloy_primitives::{
    map::{B256Map, Entry},
    B256,
};
use reth_trie::{HashedStorage, ProofTrieNodeV2, ProofV2Target};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{DeferredDrops, LeafUpdate};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::{debug_span, instrument};

pub(super) enum StorageTrieCommand {
    TouchSlots(Vec<B256>),
    ApplyHashedStorage(HashedStorage),
    Reveal(Vec<ProofTrieNodeV2>),
    Drive,
    ReturnTrie { tx: oneshot::Sender<(ActorTrie, DeferredDrops, Vec<B256>, HashedStorage)> },
}

pub(super) struct StorageTrieActor {
    address: B256,
    trie: ActorTrie,
    updates: B256Map<LeafUpdate>,
    needs_root: bool,
    root: Option<B256>,
    fetched_targets: B256Map<u8>,
    deferred: DeferredDrops,
    touched_slots: Vec<B256>,
    final_hashed_storage: HashedStorage,
    rx: UnboundedReceiver<StorageTrieCommand>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    proof_tx: UnboundedSender<ProofServiceCommand>,
}

impl StorageTrieActor {
    pub(super) fn new(
        address: B256,
        trie: ActorTrie,
        rx: UnboundedReceiver<StorageTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        proof_tx: UnboundedSender<ProofServiceCommand>,
    ) -> Self {
        Self {
            address,
            trie,
            updates: B256Map::default(),
            needs_root: false,
            root: None,
            fetched_targets: B256Map::default(),
            deferred: DeferredDrops::default(),
            touched_slots: Vec::new(),
            final_hashed_storage: HashedStorage::default(),
            rx,
            event_tx,
            proof_tx,
        }
    }

    #[instrument(
        name = "async_sr_storage_actor_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(address = ?self.address)
    )]
    pub(super) async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            if let StorageTrieCommand::ReturnTrie { tx } = command {
                let _ = tx.send((
                    self.trie,
                    self.deferred,
                    self.touched_slots,
                    self.final_hashed_storage,
                ));
                return;
            }

            if let Err(error) = self.on_command(command).await {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
            }
        }
    }

    async fn on_command(
        &mut self,
        command: StorageTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        let should_drive = match command {
            StorageTrieCommand::TouchSlots(slots) => {
                for slot in slots {
                    insert_leaf_update(&mut self.updates, slot, LeafUpdate::Touched);
                }
                false
            }
            StorageTrieCommand::ApplyHashedStorage(storage) => {
                if storage.wiped {
                    return Err(ParallelStateRootError::Other(
                        "async BAL sparse trie prototype does not support storage wipes yet"
                            .to_string(),
                    ));
                }

                self.final_hashed_storage.extend(&storage);

                for (slot, value) in storage.storage {
                    self.touched_slots.push(slot);
                    let encoded = if value.is_zero() {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    insert_leaf_update(&mut self.updates, slot, LeafUpdate::Changed(encoded));
                }
                self.needs_root = true;
                self.root = None;
                false
            }
            StorageTrieCommand::Reveal(mut nodes) => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
                true
            }
            StorageTrieCommand::Drive => true,
            StorageTrieCommand::ReturnTrie { .. } => unreachable!("handled by run"),
        };

        if should_drive {
            self.drive().await
        } else {
            Ok(())
        }
    }

    #[instrument(
        name = "async_sr_storage_drive",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(
            address = ?self.address,
            updates = self.updates.len(),
            needs_root = self.needs_root,
            root_cached = self.root.is_some(),
        )
    )]
    async fn drive(&mut self) -> Result<(), ParallelStateRootError> {
        loop {
            if self.updates.is_empty() {
                if self.needs_root && self.root.is_none() {
                    let _span = debug_span!(
                        target: "engine::tree::payload_processor::async_sparse_trie",
                        "async_sr_storage_root",
                        address = ?self.address,
                    )
                    .entered();
                    let root = self.trie.root().ok_or_else(|| {
                        ParallelStateRootError::Other(format!(
                            "storage trie for {:?} remained blind after updates drained",
                            self.address
                        ))
                    })?;
                    self.root = Some(root);
                    let _ = self
                        .event_tx
                        .send(CoordinatorEvent::StorageRootReady { address: self.address, root });
                }
                return Ok(())
            }

            let updates_len_before = self.updates.len();
            let mut chunk = drain_update_chunk(&mut self.updates, MAX_STORAGE_UPDATES_PER_DRIVE);
            let mut targets = Vec::new();
            let mut emitted_targets = 0usize;
            self.trie
                .update_leaves(&mut chunk, |path, min_len| {
                    emitted_targets += 1;
                    match self.fetched_targets.entry(path) {
                        Entry::Occupied(mut entry) => {
                            if min_len < *entry.get() {
                                entry.insert(min_len);
                                targets.push(ProofV2Target::new(path).with_min_len(min_len));
                            }
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(min_len);
                            targets.push(ProofV2Target::new(path).with_min_len(min_len));
                        }
                    }
                })
                .map_err(to_parallel_sparse_error)?;
            merge_leaf_updates(&mut self.updates, chunk);

            if !targets.is_empty() {
                self.proof_tx
                    .send(ProofServiceCommand::StorageTargets { address: self.address, targets })
                    .map_err(|_| {
                        ParallelStateRootError::Other(
                            "async sparse trie proof service dropped".to_string(),
                        )
                    })?;
                return Ok(())
            }

            if emitted_targets > 0 {
                return Ok(())
            }

            if self.updates.len() == updates_len_before {
                return Err(ParallelStateRootError::Other(format!(
                    "storage sparse trie actor for {:?} made no progress without proof targets",
                    self.address
                )));
            }
        }
    }
}
