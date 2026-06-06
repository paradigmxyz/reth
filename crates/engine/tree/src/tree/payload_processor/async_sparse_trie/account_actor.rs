use super::{
    drain_update_chunk, encode_account_leaf_value, insert_leaf_update,
    proof_service::ProofServiceCommand, to_parallel_sparse_error, ActorTrie, CoordinatorEvent,
    PendingAccountUpdate, MAX_ACCOUNT_UPDATES_PER_DRIVE,
};
use alloy_primitives::{
    map::{B256Map, B256Set, Entry, HashMap},
    B256,
};
use alloy_rlp::Decodable;
use reth_trie::{
    Nibbles, ProofTrieNodeV2, ProofV2Target, TrieAccount, EMPTY_ROOT_HASH,
    TRIE_ACCOUNT_RLP_MAX_SIZE,
};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::{DeferredDrops, LeafUpdate, SparseTrie};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::{debug, debug_span, instrument};

pub(super) enum AccountTrieCommand {
    Touch(B256),
    Reveal {
        nodes: Vec<ProofTrieNodeV2>,
        targets: Vec<ProofV2Target>,
    },
    Drive,
    FinalizeAccounts {
        pending_accounts: B256Map<PendingAccountUpdate>,
        storage_roots: B256Map<B256>,
    },
    ReturnTrie {
        tx: oneshot::Sender<(ActorTrie, DeferredDrops)>,
    },
}

pub(super) struct AccountTrieActor {
    trie: ActorTrie,
    updates: B256Map<LeafUpdate>,
    blocked_updates: HashMap<BlockedAccountTarget, B256Map<LeafUpdate>>,
    blocked_update_targets: B256Map<Vec<BlockedAccountTarget>>,
    fetched_targets: B256Map<u8>,
    finalize: Option<AccountFinalize>,
    final_updates_built: bool,
    finalized: bool,
    account_rlp_buf: Vec<u8>,
    deferred: DeferredDrops,
    rx: UnboundedReceiver<AccountTrieCommand>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    proof_tx: UnboundedSender<ProofServiceCommand>,
}

impl AccountTrieActor {
    pub(super) fn new(
        trie: ActorTrie,
        rx: UnboundedReceiver<AccountTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        proof_tx: UnboundedSender<ProofServiceCommand>,
    ) -> Self {
        Self {
            trie,
            updates: B256Map::default(),
            blocked_updates: HashMap::default(),
            blocked_update_targets: B256Map::default(),
            fetched_targets: B256Map::default(),
            finalize: None,
            final_updates_built: false,
            finalized: false,
            account_rlp_buf: Vec::with_capacity(TRIE_ACCOUNT_RLP_MAX_SIZE),
            deferred: DeferredDrops::default(),
            rx,
            event_tx,
            proof_tx,
        }
    }

    #[instrument(
        name = "async_sr_account_actor_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    pub(super) async fn run(mut self) {
        while let Some(command) = self.rx.recv().await {
            if let AccountTrieCommand::ReturnTrie { tx } = command {
                let _ = tx.send((self.trie, self.deferred));
                return;
            }

            if let Err(error) = self.on_command(command).await {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
            }
        }
    }

    async fn on_command(
        &mut self,
        command: AccountTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        let should_drive = match command {
            AccountTrieCommand::Touch(address) => {
                self.insert_account_update(address, LeafUpdate::Touched);
                false
            }
            AccountTrieCommand::Reveal { mut nodes, targets } => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
                self.promote_blocked_account_targets(targets);
                true
            }
            AccountTrieCommand::Drive => true,
            AccountTrieCommand::FinalizeAccounts { pending_accounts, storage_roots } => {
                self.finalize = Some(AccountFinalize { pending_accounts, storage_roots });
                true
            }
            AccountTrieCommand::ReturnTrie { .. } => unreachable!("handled by run"),
        };

        if should_drive {
            self.drive().await
        } else {
            Ok(())
        }
    }

    #[instrument(
        name = "async_sr_account_drive",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all,
        fields(
            updates = self.updates.len(),
            blocked_updates = self.blocked_update_count(),
            blocked_targets = self.blocked_updates.len(),
            finalize = self.finalize.is_some(),
            final_updates_built = self.final_updates_built,
            finalized = self.finalized,
            chunks = tracing::field::Empty,
            applied = tracing::field::Empty,
            blocked = tracing::field::Empty,
            targets = tracing::field::Empty,
            return_reason = tracing::field::Empty,
        )
    )]
    async fn drive(&mut self) -> Result<(), ParallelStateRootError> {
        let span = tracing::Span::current();
        self.try_build_final_updates()?;

        let mut chunks = 0usize;
        let mut applied = 0usize;
        let mut blocked = 0usize;
        let mut targets_total = 0usize;

        loop {
            if self.updates.is_empty() {
                self.try_build_final_updates()?;
                if self.has_blocked_updates() {
                    self.record_drive_summary(
                        &span,
                        chunks,
                        applied,
                        blocked,
                        targets_total,
                        "waiting_for_reveal",
                    );
                    return Ok(())
                }
                if self.updates.is_empty() && self.final_updates_built && !self.finalized {
                    self.finalized = true;
                    let _ = self.event_tx.send(CoordinatorEvent::AccountFinalized);
                    self.record_drive_summary(
                        &span,
                        chunks,
                        applied,
                        blocked,
                        targets_total,
                        "finalized",
                    );
                } else {
                    self.record_drive_summary(
                        &span,
                        chunks,
                        applied,
                        blocked,
                        targets_total,
                        "empty",
                    );
                }
                return Ok(())
            }

            let updates_len_before = self.updates.len();
            let mut chunk = drain_update_chunk(&mut self.updates, MAX_ACCOUNT_UPDATES_PER_DRIVE);
            let chunk_len_before = chunk.len();
            let mut blocked_targets = B256Map::<Vec<BlockedAccountTarget>>::default();
            let mut targets = Vec::new();
            self.trie
                .update_leaves_2(&mut chunk, |update_key, target_key, min_len| {
                    let target = BlockedAccountTarget { key: target_key, min_len };
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
            chunks += 1;
            applied += chunk_len_before.saturating_sub(chunk.len());
            blocked += chunk.len();
            targets_total += targets.len();

            let chunk_blocked = !chunk.is_empty();
            if chunk_blocked {
                self.block_account_updates(chunk, blocked_targets)?;
            }

            if !targets.is_empty() {
                self.record_drive_summary(
                    &span,
                    chunks,
                    applied,
                    blocked,
                    targets_total,
                    "proof_targets",
                );
                self.proof_tx.send(ProofServiceCommand::AccountTargets(targets)).map_err(|_| {
                    ParallelStateRootError::Other(
                        "async sparse trie proof service dropped".to_string(),
                    )
                })?;
                return Ok(())
            }

            if chunk_blocked {
                self.record_drive_summary(
                    &span,
                    chunks,
                    applied,
                    blocked,
                    targets_total,
                    "waiting_for_reveal",
                );
                return Ok(())
            }

            if self.updates.len() == updates_len_before {
                self.record_drive_summary(
                    &span,
                    chunks,
                    applied,
                    blocked,
                    targets_total,
                    "no_progress",
                );
                return Err(ParallelStateRootError::Other(
                    "account sparse trie actor made no progress without proof targets".to_string(),
                ));
            }
        }
    }

    fn record_drive_summary(
        &self,
        span: &tracing::Span,
        chunks: usize,
        applied: usize,
        blocked: usize,
        targets: usize,
        return_reason: &'static str,
    ) {
        span.record("chunks", chunks);
        span.record("applied", applied);
        span.record("blocked", blocked);
        span.record("targets", targets);
        span.record("return_reason", return_reason);
        debug!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            chunks,
            applied,
            blocked,
            targets,
            return_reason,
            updates_remaining = self.updates.len(),
            blocked_updates = self.blocked_update_count(),
            blocked_targets = self.blocked_updates.len(),
            finalize = self.finalize.is_some(),
            final_updates_built = self.final_updates_built,
            finalized = self.finalized,
            "async sparse trie account drive summary"
        );
    }

    fn insert_account_update(&mut self, key: B256, update: LeafUpdate) {
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

    fn block_account_updates(
        &mut self,
        chunk: B256Map<LeafUpdate>,
        mut blocked_targets: B256Map<Vec<BlockedAccountTarget>>,
    ) -> Result<(), ParallelStateRootError> {
        for (key, update) in chunk {
            let Some(targets) = blocked_targets.remove(&key) else {
                return Err(ParallelStateRootError::Other(
                    "account sparse trie actor retained blocked update without target metadata"
                        .to_string(),
                ))
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

    fn promote_blocked_account_targets(&mut self, targets: Vec<ProofV2Target>) {
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
                                continue
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

    fn blocked_update_count(&self) -> usize {
        self.blocked_update_targets.len()
    }

    fn try_build_final_updates(&mut self) -> Result<(), ParallelStateRootError> {
        if self.final_updates_built || !self.updates.is_empty() || self.has_blocked_updates() {
            return Ok(())
        }

        let Some(AccountFinalize { pending_accounts, storage_roots }) = self.finalize.take() else {
            return Ok(())
        };
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_build_account_final_updates",
            pending_accounts = pending_accounts.len(),
            storage_roots = storage_roots.len(),
        )
        .entered();

        for (address, pending) in pending_accounts {
            let trie_account = self
                .trie
                .as_revealed_ref()
                .and_then(|trie| trie.get_leaf_value(&Nibbles::unpack(address)))
                .map(|value| TrieAccount::decode(&mut &value[..]))
                .transpose()?;

            let storage_root = storage_roots
                .get(&address)
                .copied()
                .or_else(|| trie_account.as_ref().map(|account| account.storage_root))
                .unwrap_or(EMPTY_ROOT_HASH);

            let account = match pending {
                PendingAccountUpdate::StorageOnly => trie_account.map(Into::into),
                PendingAccountUpdate::AccountChanged(account) => account,
            };

            let encoded =
                encode_account_leaf_value(account, storage_root, &mut self.account_rlp_buf);
            self.insert_account_update(address, LeafUpdate::Changed(encoded));
        }

        self.final_updates_built = true;

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BlockedAccountTarget {
    key: B256,
    min_len: u8,
}

impl From<ProofV2Target> for BlockedAccountTarget {
    fn from(target: ProofV2Target) -> Self {
        Self { key: target.key(), min_len: target.min_len }
    }
}

struct AccountFinalize {
    pending_accounts: B256Map<PendingAccountUpdate>,
    storage_roots: B256Map<B256>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::payload_processor::async_sparse_trie::proof_service::ProofServiceCommand;
    use reth_trie_sparse::{ArenaParallelSparseTrie, ConfigurableSparseTrie, RevealableSparseTrie};
    use tokio::sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver};

    fn blind_actor() -> (
        AccountTrieActor,
        UnboundedReceiver<ProofServiceCommand>,
        UnboundedReceiver<CoordinatorEvent>,
    ) {
        let trie = RevealableSparseTrie::blind_from(ConfigurableSparseTrie::Arena(
            ArenaParallelSparseTrie::default(),
        ));
        let (_command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = unbounded_channel();
        let (proof_tx, proof_rx) = unbounded_channel();

        (AccountTrieActor::new(trie, command_rx, event_tx, proof_tx), proof_rx, event_rx)
    }

    #[tokio::test]
    async fn drive_does_not_retry_blocked_account_update_without_reveal() {
        let (mut actor, mut proof_rx, _event_rx) = blind_actor();
        let address = B256::with_last_byte(1);

        actor.on_command(AccountTrieCommand::Touch(address)).await.unwrap();
        actor.on_command(AccountTrieCommand::Drive).await.unwrap();
        let first_targets = match proof_rx.recv().await.unwrap() {
            ProofServiceCommand::AccountTargets(targets) => targets,
            _ => panic!("expected account proof targets"),
        };
        assert!(!first_targets.is_empty());
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_updates.len(), 1);

        actor.on_command(AccountTrieCommand::Drive).await.unwrap();
        assert!(matches!(proof_rx.try_recv(), Err(TryRecvError::Empty)));
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_updates.len(), 1);
    }

    #[tokio::test]
    async fn drive_filters_account_target_already_requested_by_actor() {
        let (mut actor, mut proof_rx, _event_rx) = blind_actor();
        let address = B256::with_last_byte(1);

        actor.fetched_targets.insert(address, 0);
        actor.on_command(AccountTrieCommand::Touch(address)).await.unwrap();
        actor.on_command(AccountTrieCommand::Drive).await.unwrap();

        assert!(matches!(proof_rx.try_recv(), Err(TryRecvError::Empty)));
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_update_count(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);
    }

    #[tokio::test]
    async fn drive_processes_ready_account_updates_while_other_updates_are_blocked() {
        let (mut actor, mut proof_rx, _event_rx) = blind_actor();
        let blocked_address = B256::with_last_byte(1);
        let ready_address = B256::with_last_byte(2);

        actor.on_command(AccountTrieCommand::Touch(blocked_address)).await.unwrap();
        actor.on_command(AccountTrieCommand::Drive).await.unwrap();
        assert!(matches!(proof_rx.recv().await.unwrap(), ProofServiceCommand::AccountTargets(_)));
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_updates.len(), 1);

        actor.on_command(AccountTrieCommand::Touch(ready_address)).await.unwrap();
        assert_eq!(actor.updates.len(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);

        actor.on_command(AccountTrieCommand::Drive).await.unwrap();
        assert!(matches!(proof_rx.recv().await.unwrap(), ProofServiceCommand::AccountTargets(_)));
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_updates.len(), 2);
    }

    #[tokio::test]
    async fn returned_account_target_promotes_only_matching_blocked_bucket() {
        let (mut actor, mut proof_rx, _event_rx) = blind_actor();
        let first_address = B256::with_last_byte(1);
        let second_address = B256::with_last_byte(2);

        actor.on_command(AccountTrieCommand::Touch(first_address)).await.unwrap();
        actor.on_command(AccountTrieCommand::Touch(second_address)).await.unwrap();
        actor.on_command(AccountTrieCommand::Drive).await.unwrap();

        let targets = match proof_rx.recv().await.unwrap() {
            ProofServiceCommand::AccountTargets(targets) => targets,
            _ => panic!("expected account proof targets"),
        };
        assert_eq!(targets.len(), 2);
        assert!(actor.updates.is_empty());
        assert_eq!(actor.blocked_update_count(), 2);
        assert_eq!(actor.blocked_updates.len(), 2);

        actor.promote_blocked_account_targets(vec![ProofV2Target::new(first_address)]);

        assert!(actor.updates.contains_key(&first_address));
        assert!(!actor.updates.contains_key(&second_address));
        assert_eq!(actor.blocked_update_count(), 1);
        assert_eq!(actor.blocked_updates.len(), 1);
    }

    #[test]
    fn returned_account_target_promotes_update_blocked_by_multiple_targets() {
        let (mut actor, _proof_rx, _event_rx) = blind_actor();
        let address = B256::with_last_byte(1);
        let first_target = BlockedAccountTarget { key: B256::with_last_byte(2), min_len: 3 };
        let second_target = BlockedAccountTarget { key: B256::with_last_byte(3), min_len: 4 };

        actor
            .block_account_updates(
                B256Map::from_iter([(address, LeafUpdate::Touched)]),
                B256Map::from_iter([(address, vec![first_target, second_target])]),
            )
            .unwrap();
        assert_eq!(actor.blocked_update_count(), 1);
        assert_eq!(actor.blocked_updates.len(), 2);

        actor.promote_blocked_account_targets(vec![
            ProofV2Target::new(first_target.key).with_min_len(first_target.min_len)
        ]);

        assert!(actor.updates.contains_key(&address));
        assert_eq!(actor.blocked_update_count(), 0);
        assert!(actor.blocked_updates.is_empty());
    }
}
