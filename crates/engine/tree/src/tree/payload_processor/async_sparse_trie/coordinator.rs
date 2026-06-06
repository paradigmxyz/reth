use super::{
    account_actor::{AccountTrieActor, AccountTrieCommand},
    bridge::{spawn_proof_result_bridge, spawn_state_message_bridge, BridgeHandle},
    proof_service::{ProofService, ProofServiceCommand},
    send_storage_command,
    storage_actor::{StorageTrieActor, StorageTrieCommand},
    CoordinatorEvent, PendingAccountUpdate, StateTrie,
};
use crate::tree::payload_processor::multiproof::{
    MultiProofTaskMetrics, StateRootComputeOutcome, StateRootMessage,
};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie::{updates::TrieUpdates, HashedPostState, MultiProofTargetsV2};
use reth_trie_parallel::{proof_task::ProofWorkerHandle, root::ParallelStateRootError};
use reth_trie_sparse::{
    errors::{SparseStateTrieErrorKind, SparseTrieErrorKind},
    DeferredDrops,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::{debug, debug_span, instrument, trace_span, Instrument};

pub(super) struct CoordinatorOutput {
    pub(super) result: Result<StateRootComputeOutcome, ParallelStateRootError>,
    pub(super) trie: StateTrie,
    pub(super) deferred: DeferredDrops,
}

pub(super) struct AsyncSparseTrieCoordinator {
    trie: StateTrie,
    parent_state_root: B256,
    state_rx: UnboundedReceiver<StateRootMessage>,
    state_bridge: Option<BridgeHandle>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    event_rx: UnboundedReceiver<CoordinatorEvent>,
    account_tx: UnboundedSender<AccountTrieCommand>,
    proof_tx: Option<UnboundedSender<ProofServiceCommand>>,
    proof_bridge: Option<BridgeHandle>,
    storage_actors: B256Map<StorageActorHandle>,
    pending_storage_roots: B256Set,
    storage_roots: B256Map<B256>,
    pending_accounts: B256Map<PendingAccountUpdate>,
    finished_state_updates: bool,
    finalize_sent: bool,
    metrics: MultiProofTaskMetrics,
    started_at: Instant,
}

impl AsyncSparseTrieCoordinator {
    pub(super) fn new(
        mut trie: StateTrie,
        from_multi_proof: CrossbeamReceiver<StateRootMessage>,
        proof_worker_handle: ProofWorkerHandle,
        metrics: MultiProofTaskMetrics,
        parent_state_root: B256,
        chunk_size: usize,
    ) -> Self {
        let (state_tx, state_rx) = unbounded_channel();
        let state_bridge = spawn_state_message_bridge(from_multi_proof, state_tx);

        let (event_tx, event_rx) = unbounded_channel();
        let (account_tx, account_rx) = unbounded_channel();

        let (proof_tx, proof_rx) = unbounded_channel();
        let (proof_result_tx, proof_result_rx) = crossbeam_channel::unbounded();
        let (proof_event_tx, proof_event_rx) = unbounded_channel();
        let proof_bridge = spawn_proof_result_bridge(proof_result_rx, proof_event_tx);

        let account_trie = trie.take_accounts_trie();
        let account_actor =
            AccountTrieActor::new(account_trie, account_rx, event_tx.clone(), proof_tx.clone());
        tokio::spawn(account_actor.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_account_actor"
        )));

        let proof_service = ProofService::new(
            proof_worker_handle,
            proof_result_tx,
            proof_rx,
            proof_event_rx,
            account_tx.clone(),
            event_tx.clone(),
            chunk_size,
        );
        tokio::spawn(proof_service.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_proof_service"
        )));

        Self {
            trie,
            parent_state_root,
            state_rx,
            state_bridge: Some(state_bridge),
            event_tx,
            event_rx,
            account_tx,
            proof_tx: Some(proof_tx),
            proof_bridge: Some(proof_bridge),
            storage_actors: B256Map::default(),
            pending_storage_roots: B256Set::default(),
            storage_roots: B256Map::default(),
            pending_accounts: B256Map::default(),
            finished_state_updates: false,
            finalize_sent: false,
            metrics,
            started_at: Instant::now(),
        }
    }

    #[instrument(
        name = "async_sr_coordinator",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    pub(super) async fn run(mut self) -> CoordinatorOutput {
        let mut progress_interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                maybe_message = self.state_rx.recv(), if !self.finished_state_updates => {
                    let Some(message) = maybe_message else {
                        return self.finish_with_error(ParallelStateRootError::Other(
                            "async sparse trie update stream closed before FinishedStateUpdates".to_string(),
                        )).await;
                    };

                    if let Err(error) = self.on_state_root_message(message).await {
                        return self.finish_with_error(error).await;
                    }
                }
                maybe_event = self.event_rx.recv() => {
                    let Some(event) = maybe_event else {
                        return self.finish_with_error(ParallelStateRootError::Other(
                            "async sparse trie actor event stream closed".to_string(),
                        )).await;
                    };

                    match event {
                        CoordinatorEvent::StorageRootReady { address, root } => {
                            self.storage_roots.insert(address, root);
                            if let Err(error) = self.maybe_finalize_accounts() {
                                return self.finish_with_error(error).await;
                            }
                        }
                        CoordinatorEvent::AccountFinalized => {
                            return self.finish_success().await;
                        }
                        CoordinatorEvent::Error(error) => {
                            return self.finish_with_error(error).await;
                        }
                    }
                }
                _ = progress_interval.tick(), if self.finished_state_updates => {
                    self.log_progress();
                }
            }
        }
    }

    fn log_progress(&self) {
        let missing_storage_roots = self
            .pending_storage_roots
            .iter()
            .filter(|address| !self.storage_roots.contains_key(*address))
            .count();
        debug!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            storage_actors = self.storage_actors.len(),
            pending_storage_roots = self.pending_storage_roots.len(),
            ready_storage_roots = self.storage_roots.len(),
            missing_storage_roots,
            pending_accounts = self.pending_accounts.len(),
            finalize_sent = self.finalize_sent,
            "async sparse trie coordinator progress"
        );
    }

    async fn on_state_root_message(
        &mut self,
        message: StateRootMessage,
    ) -> Result<(), ParallelStateRootError> {
        match message {
            StateRootMessage::HashedStateUpdate(update) => {
                self.on_hashed_state_update(update).await?;
            }
            StateRootMessage::PrefetchProofs(targets) => {
                self.on_prewarm_targets(targets).await?;
            }
            StateRootMessage::FinishedStateUpdates => {
                let _span = debug_span!(
                    target: "engine::tree::payload_processor::async_sparse_trie",
                    "async_sr_finished_state_updates",
                    storage_actors = self.storage_actors.len(),
                    pending_storage_roots = self.pending_storage_roots.len(),
                    pending_accounts = self.pending_accounts.len(),
                )
                .entered();
                self.finished_state_updates = true;
                self.drive_all_actors()?;
                self.maybe_finalize_accounts()?;
            }
            StateRootMessage::StateUpdate(_, _) | StateRootMessage::BlockAccessList(_) => {
                return Err(ParallelStateRootError::Other(
                    "async BAL sparse trie prototype only accepts pre-hashed BAL updates"
                        .to_string(),
                ));
            }
        }

        Ok(())
    }

    async fn on_hashed_state_update(
        &mut self,
        hashed_state_update: HashedPostState,
    ) -> Result<(), ParallelStateRootError> {
        let accounts_len = hashed_state_update.accounts.len();
        let storages_len = hashed_state_update.storages.len();
        let storage_slots = hashed_state_update
            .storages
            .values()
            .map(|storage| storage.storage.len())
            .sum::<usize>();
        let _span = trace_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_hashed_state_update",
            accounts = accounts_len,
            storages = storages_len,
            storage_slots,
        )
        .entered();

        match (accounts_len, storages_len) {
            (0, 0) => Ok(()),
            (1, 0) => {
                let (address, account) =
                    hashed_state_update.accounts.into_iter().next().expect("checked len");
                self.trie.record_account_touch(address);
                self.pending_accounts.insert(address, PendingAccountUpdate::AccountChanged(account));
                self.send_account_command(AccountTrieCommand::Touch(address))?;
                Ok(())
            }
            (0, 1) => {
                let (address, storage) =
                    hashed_state_update.storages.into_iter().next().expect("checked len");
                if storage.wiped {
                    return Err(ParallelStateRootError::Other(
                        "async BAL sparse trie prototype does not support storage wipes yet"
                            .to_string(),
                    ));
                }

                for &slot in storage.storage.keys() {
                    self.trie.record_slot_touch(address, slot);
                }

                if !storage.storage.is_empty() {
                    self.pending_accounts
                        .entry(address)
                        .or_insert(PendingAccountUpdate::StorageOnly);
                    self.pending_storage_roots.insert(address);

                    let storage_tx = self.ensure_storage_actor(address)?;
                    send_storage_command(
                        &storage_tx,
                        StorageTrieCommand::ApplyHashedStorage(storage),
                    )?;
                    self.send_account_command(AccountTrieCommand::Touch(address))?;
                }

                Ok(())
            }
            _ => Err(ParallelStateRootError::Other(format!(
                "async BAL sparse trie expected one-account-scoped hashed update, got {accounts_len} account entries and {storages_len} storage entries",
            ))),
        }
    }

    async fn on_prewarm_targets(
        &mut self,
        targets: MultiProofTargetsV2,
    ) -> Result<(), ParallelStateRootError> {
        let account_targets = targets.account_targets.len();
        let storage_tries = targets.storage_targets.len();
        let storage_targets =
            targets.storage_targets.values().map(|targets| targets.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_prewarm_targets",
            account_targets,
            storage_tries,
            storage_targets,
        )
        .entered();

        for target in targets.account_targets {
            self.send_account_command(AccountTrieCommand::Touch(target.key()))?;
        }

        for (address, slots) in targets.storage_targets {
            if !slots.is_empty() {
                let storage_tx = self.ensure_storage_actor(address)?;
                let slots = slots.into_iter().map(|target| target.key()).collect();
                send_storage_command(&storage_tx, StorageTrieCommand::TouchSlots(slots))?;
                send_storage_command(&storage_tx, StorageTrieCommand::Drive)?;
            }
            self.send_account_command(AccountTrieCommand::Touch(address))?;
        }
        self.send_account_command(AccountTrieCommand::Drive)?;

        Ok(())
    }

    fn ensure_storage_actor(
        &mut self,
        address: B256,
    ) -> Result<UnboundedSender<StorageTrieCommand>, ParallelStateRootError> {
        if let Some(actor) = self.storage_actors.get(&address) {
            return Ok(actor.tx.clone())
        }

        let trie = self.trie.take_or_create_storage_trie(&address);
        let (tx, rx) = unbounded_channel();
        let actor =
            StorageTrieActor::new(address, trie, rx, self.event_tx.clone(), self.proof_tx()?);
        tokio::spawn(actor.run().instrument(debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_storage_actor",
            ?address,
        )));

        self.send_proof_command(ProofServiceCommand::RegisterStorageActor {
            address,
            tx: tx.clone(),
        })?;
        self.storage_actors.insert(address, StorageActorHandle { tx: tx.clone() });

        Ok(tx)
    }

    fn drive_all_actors(&self) -> Result<(), ParallelStateRootError> {
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_drive_all_actors",
            storage_actors = self.storage_actors.len(),
            pending_storage_roots = self.pending_storage_roots.len(),
            pending_accounts = self.pending_accounts.len(),
        )
        .entered();

        self.send_account_command(AccountTrieCommand::Drive)?;
        for actor in self.storage_actors.values() {
            send_storage_command(&actor.tx, StorageTrieCommand::Drive)?;
        }
        Ok(())
    }

    fn maybe_finalize_accounts(&mut self) -> Result<(), ParallelStateRootError> {
        if !self.finished_state_updates || self.finalize_sent {
            return Ok(())
        }

        if self
            .pending_storage_roots
            .iter()
            .any(|address| !self.storage_roots.contains_key(address))
        {
            return Ok(())
        }

        let pending_accounts = std::mem::take(&mut self.pending_accounts);
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_send_account_finalize",
            pending_accounts = pending_accounts.len(),
            storage_roots = self.storage_roots.len(),
        )
        .entered();
        self.send_account_command(AccountTrieCommand::FinalizeAccounts {
            pending_accounts,
            storage_roots: self.storage_roots.clone(),
        })?;
        self.finalize_sent = true;

        Ok(())
    }

    async fn finish_success(mut self) -> CoordinatorOutput {
        let storage_actors = self.storage_actors.len();
        let actor_result = self
            .return_actor_tries()
            .instrument(debug_span!(
                target: "engine::tree::payload_processor::async_sparse_trie",
                "async_sr_return_actor_tries",
                storage_actors,
            ))
            .await;
        self.shutdown_bridges();

        let result = actor_result.and_then(|_| self.compute_outcome());
        let deferred = self.trie.take_deferred_drops();

        CoordinatorOutput { result, trie: self.trie, deferred }
    }

    async fn finish_with_error(mut self, error: ParallelStateRootError) -> CoordinatorOutput {
        let storage_actors = self.storage_actors.len();
        let actor_result = self
            .return_actor_tries()
            .instrument(debug_span!(
                target: "engine::tree::payload_processor::async_sparse_trie",
                "async_sr_return_actor_tries",
                storage_actors,
            ))
            .await;
        self.shutdown_bridges();

        let result = actor_result.and(Err(error));
        let deferred = self.trie.take_deferred_drops();

        CoordinatorOutput { result, trie: self.trie, deferred }
    }

    async fn return_actor_tries(&mut self) -> Result<(), ParallelStateRootError> {
        let (account_return_tx, account_return_rx) = oneshot::channel();
        self.send_account_command(AccountTrieCommand::ReturnTrie { tx: account_return_tx })?;
        let (account_trie, account_deferred) = account_return_rx.await.map_err(|_| {
            ParallelStateRootError::Other(
                "async account sparse trie actor dropped before returning trie".to_string(),
            )
        })?;
        self.trie.insert_accounts_trie(account_trie);
        self.trie.extend_deferred_drops(account_deferred);

        for (address, actor) in std::mem::take(&mut self.storage_actors) {
            let (return_tx, return_rx) = oneshot::channel();
            send_storage_command(&actor.tx, StorageTrieCommand::ReturnTrie { tx: return_tx })?;
            let (storage_trie, storage_deferred) = return_rx.await.map_err(|_| {
                ParallelStateRootError::Other(format!(
                    "async storage sparse trie actor for {address:?} dropped before returning trie"
                ))
            })?;
            self.trie.insert_storage_trie(address, storage_trie);
            self.trie.extend_deferred_drops(storage_deferred);
        }

        self.proof_tx.take();

        Ok(())
    }

    fn compute_outcome(&mut self) -> Result<StateRootComputeOutcome, ParallelStateRootError> {
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_compute_outcome"
        )
        .entered();
        let start = Instant::now();
        let (state_root, trie_updates) = match self.trie.root_with_updates() {
            Ok(result) => result,
            Err(err)
                if matches!(
                    err.kind(),
                    SparseStateTrieErrorKind::Sparse(SparseTrieErrorKind::Blind)
                ) =>
            {
                (self.parent_state_root, TrieUpdates::default())
            }
            Err(err) => {
                return Err(ParallelStateRootError::Other(format!(
                    "could not calculate async sparse trie state root: {err:?}"
                )))
            }
        };

        #[cfg(feature = "trie-debug")]
        let debug_recorders = self.trie.take_debug_recorders();

        let end = Instant::now();
        self.metrics.sparse_trie_final_update_duration_histogram.record(end.duration_since(start));
        self.metrics
            .sparse_trie_total_duration_histogram
            .record(end.duration_since(self.started_at));

        Ok(StateRootComputeOutcome {
            state_root,
            trie_updates: Arc::new(trie_updates),
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        })
    }

    fn shutdown_bridges(&mut self) {
        self.proof_tx.take();
        if let Some(bridge) = self.state_bridge.take() {
            bridge.stop();
        }
        if let Some(bridge) = self.proof_bridge.take() {
            bridge.stop();
        }
    }

    fn proof_tx(&self) -> Result<UnboundedSender<ProofServiceCommand>, ParallelStateRootError> {
        self.proof_tx.clone().ok_or_else(|| {
            ParallelStateRootError::Other("async sparse trie proof service stopped".to_string())
        })
    }

    fn send_proof_command(
        &self,
        command: ProofServiceCommand,
    ) -> Result<(), ParallelStateRootError> {
        self.proof_tx()?.send(command).map_err(|_| {
            ParallelStateRootError::Other("async sparse trie proof service dropped".to_string())
        })
    }

    fn send_account_command(
        &self,
        command: AccountTrieCommand,
    ) -> Result<(), ParallelStateRootError> {
        self.account_tx.send(command).map_err(|_| {
            ParallelStateRootError::Other("async account sparse trie actor dropped".to_string())
        })
    }
}

struct StorageActorHandle {
    tx: UnboundedSender<StorageTrieCommand>,
}
