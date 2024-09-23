use futures::{FutureExt, StreamExt};
use reth_provider::{providers::ConsistentDbView, BlockReader, DatabaseProviderFactory};
use reth_tasks::pool::BlockingTaskPool;
use reth_trie::{
    prefix_set::TriePrefixSetsMut, updates::TrieUpdates, HashedPostState, HashedStorage, TrieInput,
};
use reth_trie_parallel::async_root::{AsyncStateRoot, AsyncStateRootError};
use revm_primitives::{keccak256, B256};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

pub type AsyncStateRootFut =
    Pin<Box<dyn Future<Output = Result<(B256, TrieUpdates), AsyncStateRootError>> + Send>>;

pub struct StateRootTask<Factory> {
    consistent_view: ConsistentDbView<Factory>,
    blocking_task_pool: BlockingTaskPool,
    input: TrieInput,
    state_stream: UnboundedReceiverStream<revm_primitives::EvmState>,
    state_stream_closed: bool,
    state: HashedPostState,
    prefix_sets: TriePrefixSetsMut,
    trie_updates: TrieUpdates,
    pending_state_root: Option<AsyncStateRootFut>,
    state_root: B256,
}

impl<Factory> StateRootTask<Factory> {
    pub fn new(
        consistent_view: ConsistentDbView<Factory>,
        input: TrieInput,
        state_stream: UnboundedReceiverStream<revm_primitives::EvmState>,
        parent_state_root: B256,
    ) -> Self {
        Self {
            consistent_view,
            blocking_task_pool: BlockingTaskPool::build().unwrap(),
            input,
            state_stream,
            state_stream_closed: false,
            state: HashedPostState::default(),
            prefix_sets: TriePrefixSetsMut::default(),
            trie_updates: TrieUpdates::default(),
            pending_state_root: None,
            state_root: parent_state_root,
        }
    }

    fn on_state_update(&mut self, update: revm_primitives::EvmState) {
        let mut hashed_state_update = HashedPostState::default();
        for (address, account) in update {
            if account.is_touched() {
                let hashed_address = keccak256(address);

                let destroyed = account.is_selfdestructed();
                hashed_state_update.accounts.insert(
                    hashed_address,
                    if destroyed || account.is_empty() { None } else { Some(account.info.into()) },
                );

                if destroyed || !account.storage.is_empty() {
                    let storage = account.storage.into_iter().filter_map(|(slot, value)| {
                        (!destroyed && value.is_changed())
                            .then(|| (keccak256(B256::from(slot)), value.present_value))
                    });
                    hashed_state_update
                        .storages
                        .insert(hashed_address, HashedStorage::from_iter(destroyed, storage));
                }
            }
        }
        self.prefix_sets.extend(hashed_state_update.construct_prefix_sets());
        self.state.extend(hashed_state_update);
    }
}

impl<Factory> Future for StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + Unpin + 'static,
{
    type Output = Result<(B256, TrieUpdates), AsyncStateRootError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            if this.state_stream_closed &&
                this.prefix_sets.is_empty() &&
                this.pending_state_root.is_none()
            {
                return Poll::Ready(Ok((this.state_root, std::mem::take(&mut this.trie_updates))))
            }

            if let Poll::Ready(next) = this.state_stream.poll_next_unpin(cx) {
                if let Some(update) = next {
                    debug!(target: "engine::root", len = update.len(), "Received new state update");
                    this.on_state_update(update);
                    continue
                } else {
                    this.state_stream_closed = true;
                }
            }

            if let Some(mut pending) = this.pending_state_root.take() {
                match pending.poll_unpin(cx)? {
                    Poll::Ready((state_root, trie_updates)) => {
                        debug!(target: "engine::root", %state_root, "Computed intermediate root");
                        this.state_root = state_root;
                        this.trie_updates.extend(trie_updates);
                        continue
                    }
                    Poll::Pending => {
                        this.pending_state_root = Some(pending);
                    }
                }
            }

            if this.pending_state_root.is_none() && !this.prefix_sets.is_empty() {
                debug!(target: "engine::root", accounts_len = this.prefix_sets.account_prefix_set.len(), "Spawning state root task");
                let view = this.consistent_view.clone();
                let task_pool = this.blocking_task_pool.clone();
                let mut input = this.input.clone(); // TODO: avoid cloning?
                input.nodes.extend_ref(&this.trie_updates);
                input.state.extend_ref(&this.state);
                input.prefix_sets.extend(std::mem::take(&mut this.prefix_sets));
                let (tx, rx) = oneshot::channel();
                rayon::spawn(|| {
                    let result = futures::executor::block_on(async move {
                        AsyncStateRoot::new(view, task_pool, input)
                            .incremental_root_with_updates()
                            .await
                    });
                    let _ = tx.send(result);
                });
                this.pending_state_root = Some(Box::pin(async move { rx.await.unwrap() }));
                continue
            }

            return Poll::Pending
        }
    }
}
