use super::{
    account_actor::AccountTrieCommand, storage_actor::StorageTrieCommand, CoordinatorEvent,
};
use crate::tree::payload_processor::multiproof::{
    dispatch_with_chunking, DEFAULT_MAX_TARGETS_FOR_CHUNKING,
};
use alloy_primitives::{
    map::{B256Map, Entry},
    B256,
};
use crossbeam_channel::Sender as CrossbeamSender;
use reth_primitives_traits::FastInstant as Instant;
use reth_trie::{DecodedMultiProofV2, HashedPostState, MultiProofTargetsV2, ProofV2Target};
use reth_trie_parallel::{
    proof_task::{
        AccountMultiproofInput, ProofResultContext, ProofResultMessage, ProofWorkerHandle,
    },
    root::ParallelStateRootError,
};
use tokio::sync::mpsc::{error::TryRecvError, UnboundedReceiver, UnboundedSender};
use tracing::{debug_span, error, instrument};

pub(super) enum ProofServiceCommand {
    AccountTargets(Vec<ProofV2Target>),
    StorageTargets { address: B256, targets: Vec<ProofV2Target> },
    RegisterStorageActor { address: B256, tx: UnboundedSender<StorageTrieCommand> },
}

pub(super) struct ProofService {
    proof_worker_handle: ProofWorkerHandle,
    proof_result_tx: CrossbeamSender<ProofResultMessage>,
    command_rx: UnboundedReceiver<ProofServiceCommand>,
    proof_rx: UnboundedReceiver<ProofResultMessage>,
    account_tx: UnboundedSender<AccountTrieCommand>,
    storage_txs: B256Map<UnboundedSender<StorageTrieCommand>>,
    event_tx: UnboundedSender<CoordinatorEvent>,
    pending_targets: PendingTargets,
    fetched_account_targets: B256Map<u8>,
    fetched_storage_targets: B256Map<B256Map<u8>>,
    chunk_size: usize,
    max_targets_for_chunking: usize,
    inflight: usize,
    commands_closed: bool,
}

impl ProofService {
    #[expect(clippy::too_many_arguments)]
    pub(super) fn new(
        proof_worker_handle: ProofWorkerHandle,
        proof_result_tx: CrossbeamSender<ProofResultMessage>,
        command_rx: UnboundedReceiver<ProofServiceCommand>,
        proof_rx: UnboundedReceiver<ProofResultMessage>,
        account_tx: UnboundedSender<AccountTrieCommand>,
        event_tx: UnboundedSender<CoordinatorEvent>,
        chunk_size: usize,
    ) -> Self {
        Self {
            proof_worker_handle,
            proof_result_tx,
            command_rx,
            proof_rx,
            account_tx,
            storage_txs: B256Map::default(),
            event_tx,
            pending_targets: PendingTargets::default(),
            fetched_account_targets: B256Map::default(),
            fetched_storage_targets: B256Map::default(),
            chunk_size,
            max_targets_for_chunking: DEFAULT_MAX_TARGETS_FOR_CHUNKING,
            inflight: 0,
            commands_closed: false,
        }
    }

    #[instrument(
        name = "async_sr_proof_service_run",
        level = "debug",
        target = "engine::tree::payload_processor::async_sparse_trie",
        skip_all
    )]
    pub(super) async fn run(mut self) {
        loop {
            if self.commands_closed && !self.pending_targets.is_empty() {
                self.dispatch_pending_targets();
                continue;
            }

            if self.commands_closed && self.inflight == 0 && self.pending_targets.is_empty() {
                return
            }

            tokio::select! {
                maybe_command = self.command_rx.recv(), if !self.commands_closed => {
                    match maybe_command {
                        Some(command) => {
                            self.on_command(command);
                            self.drain_ready_commands();
                            if !self.pending_targets.is_empty() {
                                tokio::task::yield_now().await;
                                self.drain_ready_commands();
                                self.dispatch_pending_targets();
                            }
                        }
                        None => self.commands_closed = true,
                    }
                }
                maybe_proof = self.proof_rx.recv(), if self.inflight > 0 => {
                    match maybe_proof {
                        Some(proof) => self.on_proof_result(proof),
                        None => {
                            let _ = self.event_tx.send(CoordinatorEvent::Error(
                                ParallelStateRootError::Other(
                                    "async sparse trie proof result bridge dropped".to_string(),
                                ),
                            ));
                            return;
                        }
                    }
                }
            }
        }
    }

    fn drain_ready_commands(&mut self) {
        loop {
            match self.command_rx.try_recv() {
                Ok(command) => self.on_command(command),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.commands_closed = true;
                    return
                }
            }
        }
    }

    fn on_command(&mut self, command: ProofServiceCommand) {
        match command {
            ProofServiceCommand::AccountTargets(targets) => {
                self.queue_account_targets(targets);
            }
            ProofServiceCommand::StorageTargets { address, targets } => {
                self.queue_storage_targets(address, targets);
            }
            ProofServiceCommand::RegisterStorageActor { address, tx } => {
                self.storage_txs.insert(address, tx);
            }
        }
    }

    fn queue_account_targets(&mut self, targets: Vec<ProofV2Target>) {
        let mut queued = 0usize;
        let mut retry_if_idle = Vec::new();

        for target in targets {
            match self.fetched_account_targets.entry(target.key()) {
                Entry::Occupied(mut entry) => {
                    if target.min_len < *entry.get() {
                        entry.insert(target.min_len);
                        self.pending_targets.push_account_target(target);
                        queued += 1;
                    } else {
                        retry_if_idle.push(target);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(target.min_len);
                    self.pending_targets.push_account_target(target);
                    queued += 1;
                }
            }
        }

        if queued == 0 && self.inflight == 0 && self.pending_targets.is_empty() {
            for target in retry_if_idle {
                self.fetched_account_targets.insert(target.key(), target.min_len);
                self.pending_targets.push_account_target(target);
            }
        }
    }

    fn queue_storage_targets(&mut self, address: B256, targets: Vec<ProofV2Target>) {
        let fetched = self.fetched_storage_targets.entry(address).or_default();
        let mut queued = Vec::new();
        let mut retry_if_idle = Vec::new();
        for target in targets {
            match fetched.entry(target.key()) {
                Entry::Occupied(mut entry) => {
                    if target.min_len < *entry.get() {
                        entry.insert(target.min_len);
                        queued.push(target);
                    } else {
                        retry_if_idle.push(target);
                    }
                }
                Entry::Vacant(entry) => {
                    entry.insert(target.min_len);
                    queued.push(target);
                }
            }
        }

        if !queued.is_empty() {
            self.pending_targets.extend_storage_targets(&address, queued);
        } else if self.inflight == 0 && self.pending_targets.is_empty() && !retry_if_idle.is_empty()
        {
            let fetched = self.fetched_storage_targets.entry(address).or_default();
            for target in &retry_if_idle {
                fetched.insert(target.key(), target.min_len);
            }
            self.pending_targets.extend_storage_targets(&address, retry_if_idle);
        }
    }

    fn on_proof_result(&mut self, proof: ProofResultMessage) {
        self.inflight = self.inflight.saturating_sub(1);

        let decoded = match proof.result {
            Ok(decoded) => decoded,
            Err(error) => {
                let _ = self.event_tx.send(CoordinatorEvent::Error(error));
                return
            }
        };

        let DecodedMultiProofV2 { account_proofs, storage_proofs } = decoded;
        let storage_tries = storage_proofs.len();
        let storage_proof_nodes = storage_proofs.values().map(|nodes| nodes.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_proof_result",
            account_proof_nodes = account_proofs.len(),
            storage_tries,
            storage_proof_nodes,
            inflight = self.inflight,
        )
        .entered();

        if !account_proofs.is_empty() &&
            self.account_tx.send(AccountTrieCommand::Reveal(account_proofs)).is_err()
        {
            let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                "async account sparse trie actor dropped before proof reveal".to_string(),
            )));
        }

        for (address, proof_nodes) in storage_proofs {
            let Some(tx) = self.storage_txs.get(&address) else {
                let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                    format!("received storage proof for unregistered actor {address:?}"),
                )));
                continue;
            };

            if tx.send(StorageTrieCommand::Reveal(proof_nodes)).is_err() {
                let _ = self.event_tx.send(CoordinatorEvent::Error(ParallelStateRootError::Other(
                    format!(
                    "async storage sparse trie actor for {address:?} dropped before proof reveal"
                ),
                )));
            }
        }
    }

    fn dispatch_pending_targets(&mut self) {
        if self.pending_targets.is_empty() {
            return;
        }

        let (targets, chunking_length) = self.pending_targets.take();
        let account_targets = targets.account_targets.len();
        let storage_tries = targets.storage_targets.len();
        let storage_targets =
            targets.storage_targets.values().map(|targets| targets.len()).sum::<usize>();
        let _span = debug_span!(
            target: "engine::tree::payload_processor::async_sparse_trie",
            "async_sr_dispatch_pending_targets",
            chunking_length,
            account_targets,
            storage_tries,
            storage_targets,
            inflight = self.inflight,
        )
        .entered();
        let proof_worker_handle = self.proof_worker_handle.clone();
        let proof_result_tx = self.proof_result_tx.clone();
        let event_tx = self.event_tx.clone();
        let has_multiple_idle_account_workers =
            self.proof_worker_handle.has_multiple_idle_account_workers();
        let has_multiple_idle_storage_workers =
            self.proof_worker_handle.has_multiple_idle_storage_workers();
        let mut dispatched = 0usize;

        dispatch_with_chunking(
            targets,
            chunking_length,
            self.chunk_size,
            self.max_targets_for_chunking,
            has_multiple_idle_account_workers,
            has_multiple_idle_storage_workers,
            MultiProofTargetsV2::chunks,
            |proof_targets| {
                dispatched += 1;
                if let Err(error) =
                    proof_worker_handle.dispatch_account_multiproof(AccountMultiproofInput {
                        targets: proof_targets,
                        proof_result_sender: ProofResultContext::new(
                            proof_result_tx.clone(),
                            HashedPostState::default(),
                            Instant::now(),
                        ),
                    })
                {
                    error!("failed to dispatch async sparse trie account multiproof: {error:?}");
                    let _ = event_tx.send(CoordinatorEvent::Error(error.into()));
                }
            },
        );

        self.inflight += dispatched;
    }
}

#[derive(Default)]
struct PendingTargets {
    targets: MultiProofTargetsV2,
    len: usize,
}

impl PendingTargets {
    const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn take(&mut self) -> (MultiProofTargetsV2, usize) {
        (std::mem::take(&mut self.targets), std::mem::take(&mut self.len))
    }

    fn push_account_target(&mut self, target: ProofV2Target) {
        self.targets.account_targets.push(target);
        self.len += 1;
    }

    fn extend_storage_targets(&mut self, address: &B256, targets: Vec<ProofV2Target>) {
        self.len += targets.len();
        self.targets.storage_targets.entry(*address).or_default().extend(targets);
    }
}
