use super::{
    drain_update_chunk, encode_account_leaf_value, insert_leaf_update, merge_leaf_updates,
    proof_service::ProofServiceCommand, to_parallel_sparse_error, ActorTrie, CoordinatorEvent,
    PendingAccountUpdate, MAX_ACCOUNT_UPDATES_PER_DRIVE,
};
use alloy_primitives::{map::B256Map, B256};
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
use tracing::{debug_span, instrument};

pub(super) enum AccountTrieCommand {
    Touch(B256),
    Reveal(Vec<ProofTrieNodeV2>),
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
                insert_leaf_update(&mut self.updates, address, LeafUpdate::Touched);
                false
            }
            AccountTrieCommand::Reveal(mut nodes) => {
                self.trie
                    .reveal_v2_proof_nodes(&mut nodes, true)
                    .map_err(to_parallel_sparse_error)?;
                self.deferred.proof_nodes_bufs.push(nodes);
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
            finalize = self.finalize.is_some(),
            final_updates_built = self.final_updates_built,
            finalized = self.finalized,
        )
    )]
    async fn drive(&mut self) -> Result<(), ParallelStateRootError> {
        self.try_build_final_updates()?;

        loop {
            if self.updates.is_empty() {
                self.try_build_final_updates()?;
                if self.updates.is_empty() && self.final_updates_built && !self.finalized {
                    self.finalized = true;
                    let _ = self.event_tx.send(CoordinatorEvent::AccountFinalized);
                }
                return Ok(())
            }

            let updates_len_before = self.updates.len();
            let mut chunk = drain_update_chunk(&mut self.updates, MAX_ACCOUNT_UPDATES_PER_DRIVE);
            let mut targets = Vec::new();
            self.trie
                .update_leaves(&mut chunk, |path, min_len| {
                    targets.push(ProofV2Target::new(path).with_min_len(min_len));
                })
                .map_err(to_parallel_sparse_error)?;
            merge_leaf_updates(&mut self.updates, chunk);

            if !targets.is_empty() {
                self.proof_tx.send(ProofServiceCommand::AccountTargets(targets)).map_err(|_| {
                    ParallelStateRootError::Other(
                        "async sparse trie proof service dropped".to_string(),
                    )
                })?;
                return Ok(())
            }

            if self.updates.len() == updates_len_before {
                return Err(ParallelStateRootError::Other(
                    "account sparse trie actor made no progress without proof targets".to_string(),
                ));
            }
        }
    }

    fn try_build_final_updates(&mut self) -> Result<(), ParallelStateRootError> {
        if self.final_updates_built || !self.updates.is_empty() {
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
            self.updates.insert(address, LeafUpdate::Changed(encoded));
        }

        self.final_updates_built = true;

        Ok(())
    }
}

struct AccountFinalize {
    pending_accounts: B256Map<PendingAccountUpdate>,
    storage_roots: B256Map<B256>,
}
