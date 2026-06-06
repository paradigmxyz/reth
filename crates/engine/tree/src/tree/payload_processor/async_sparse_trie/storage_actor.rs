use super::{
    drain_update_chunk, insert_leaf_update, proof_service::ProofServiceCommand,
    to_parallel_sparse_error, ActorTrie, CoordinatorEvent, MAX_STORAGE_UPDATES_PER_DRIVE,
};
use alloy_primitives::{
    map::{B256Map, B256Set, Entry, HashMap},
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
    Reveal { nodes: Vec<ProofTrieNodeV2>, targets: Vec<ProofV2Target> },
    Drive,
    ReturnTrie { tx: oneshot::Sender<(ActorTrie, DeferredDrops, Vec<B256>, HashedStorage)> },
}

pub(super) struct StorageTrieActor {
    address: B256,
    trie: ActorTrie,
    updates: B256Map<LeafUpdate>,
    blocked_updates: HashMap<BlockedStorageTarget, B256Map<LeafUpdate>>,
    blocked_update_targets: B256Map<Vec<BlockedStorageTarget>>,
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
            blocked_updates: HashMap::default(),
            blocked_update_targets: B256Map::default(),
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
                    self.insert_storage_update(slot, LeafUpdate::Touched);
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
                    self.insert_storage_update(slot, LeafUpdate::Changed(encoded));
                }
                self.needs_root = true;
                self.root = None;
                false
            }
            StorageTrieCommand::Reveal { mut nodes, targets } => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
                self.promote_blocked_storage_targets(targets);
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
                if self.has_blocked_updates() {
                    return Ok(())
                }

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
            let mut blocked_targets = B256Map::<Vec<BlockedStorageTarget>>::default();
            let mut targets = Vec::new();
            self.trie
                .update_leaves_2(&mut chunk, |update_key, target_key, min_len| {
                    let target = BlockedStorageTarget { key: target_key, min_len };
                    blocked_targets.entry(update_key).or_default().push(target);
                    match self.fetched_targets.entry(target_key) {
                        Entry::Occupied(mut entry) => {
                            if min_len < *entry.get() {
                                entry.insert(min_len);
                                targets.push(ProofV2Target::new(target_key).with_min_len(min_len));
                            }
                        }
                        Entry::Vacant(entry) => {
                            entry.insert(min_len);
                            targets.push(ProofV2Target::new(target_key).with_min_len(min_len));
                        }
                    }
                })
                .map_err(to_parallel_sparse_error)?;

            let chunk_blocked = !chunk.is_empty();
            if chunk_blocked {
                self.block_storage_updates(chunk, blocked_targets)?;
            }

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

            if chunk_blocked {
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

    fn insert_storage_update(&mut self, key: B256, update: LeafUpdate) {
        if let Some(targets) = self.blocked_update_targets.get(&key).cloned() {
            for target in targets {
                insert_leaf_update(
                    self.blocked_updates.entry(target).or_default(),
                    key,
                    update.clone(),
                );
            }
        } else {
            insert_leaf_update(&mut self.updates, key, update);
        }
    }

    fn block_storage_updates(
        &mut self,
        chunk: B256Map<LeafUpdate>,
        mut blocked_targets: B256Map<Vec<BlockedStorageTarget>>,
    ) -> Result<(), ParallelStateRootError> {
        for (key, update) in chunk {
            let Some(targets) = blocked_targets.remove(&key) else {
                return Err(ParallelStateRootError::Other(format!(
                    "storage sparse trie actor for {:?} retained blocked update without target metadata",
                    self.address
                )))
            };
            self.blocked_update_targets.insert(key, targets.clone());
            for target in targets {
                insert_leaf_update(
                    self.blocked_updates.entry(target).or_default(),
                    key,
                    update.clone(),
                );
            }
        }

        Ok(())
    }

    fn promote_blocked_storage_targets(&mut self, targets: Vec<ProofV2Target>) {
        if targets.is_empty() {
            return
        }

        let mut promoted = B256Set::default();
        for target in targets {
            let target_key = target.key();
            let matching: Vec<_> = self
                .blocked_updates
                .keys()
                .filter(|blocked| blocked.key == target_key && target.min_len <= blocked.min_len)
                .copied()
                .collect();

            for blocked_target in matching {
                let Some(bucket) = self.blocked_updates.remove(&blocked_target) else { continue };
                for (key, update) in bucket {
                    if !promoted.insert(key) {
                        continue
                    }

                    if let Some(targets) = self.blocked_update_targets.remove(&key) {
                        for other_target in targets {
                            if other_target == blocked_target {
                                continue;
                            }

                            let remove_bucket = if let Some(other_bucket) =
                                self.blocked_updates.get_mut(&other_target)
                            {
                                other_bucket.remove(&key);
                                other_bucket.is_empty()
                            } else {
                                false
                            };
                            if remove_bucket {
                                self.blocked_updates.remove(&other_target);
                            }
                        }
                    }
                    insert_leaf_update(&mut self.updates, key, update);
                }
            }
        }
    }

    fn has_blocked_updates(&self) -> bool {
        !self.blocked_update_targets.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BlockedStorageTarget {
    key: B256,
    min_len: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::payload_processor::async_sparse_trie::proof_service::ProofServiceCommand;
    use reth_trie_sparse::{ArenaParallelSparseTrie, ConfigurableSparseTrie, RevealableSparseTrie};
    use tokio::sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver};

    fn blind_actor() -> (
        StorageTrieActor,
        UnboundedReceiver<ProofServiceCommand>,
        UnboundedReceiver<CoordinatorEvent>,
    ) {
        let trie = RevealableSparseTrie::blind_from(ConfigurableSparseTrie::Arena(
            ArenaParallelSparseTrie::default(),
        ));
        let (_command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = unbounded_channel();
        let (proof_tx, proof_rx) = unbounded_channel();

        (
            StorageTrieActor::new(B256::with_last_byte(1), trie, command_rx, event_tx, proof_tx),
            proof_rx,
            event_rx,
        )
    }

    #[tokio::test]
    async fn drive_does_not_retry_blocked_storage_update_without_reveal() {
        let (mut actor, mut proof_rx, _event_rx) = blind_actor();
        let slot = B256::with_last_byte(2);

        actor.on_command(StorageTrieCommand::TouchSlots(vec![slot])).await.unwrap();
        actor.on_command(StorageTrieCommand::Drive).await.unwrap();
        let targets = match proof_rx.recv().await.unwrap() {
            ProofServiceCommand::StorageTargets { targets, .. } => targets,
            _ => panic!("expected storage proof targets"),
        };
        assert_eq!(targets.len(), 1);
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_update_targets.len(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);

        actor.on_command(StorageTrieCommand::Drive).await.unwrap();
        assert!(matches!(proof_rx.try_recv(), Err(TryRecvError::Empty)));
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_update_targets.len(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);
    }

    #[test]
    fn returned_storage_target_promotes_only_matching_blocked_bucket() {
        let (mut actor, _proof_rx, _event_rx) = blind_actor();
        let first_slot = B256::with_last_byte(2);
        let second_slot = B256::with_last_byte(3);
        let first_target = BlockedStorageTarget { key: first_slot, min_len: 0 };
        let second_target = BlockedStorageTarget { key: second_slot, min_len: 0 };

        actor
            .block_storage_updates(
                B256Map::from_iter([
                    (first_slot, LeafUpdate::Touched),
                    (second_slot, LeafUpdate::Touched),
                ]),
                B256Map::from_iter([
                    (first_slot, vec![first_target]),
                    (second_slot, vec![second_target]),
                ]),
            )
            .unwrap();
        assert_eq!(actor.blocked_update_targets.len(), 2);
        assert_eq!(actor.blocked_updates.len(), 2);

        actor.promote_blocked_storage_targets(vec![ProofV2Target::new(first_slot)]);

        assert!(actor.updates.contains_key(&first_slot));
        assert!(!actor.updates.contains_key(&second_slot));
        assert_eq!(actor.blocked_update_targets.len(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);
    }
}
