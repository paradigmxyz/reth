//! Async state root task implementation.

use super::{StateRootConfig, StateRootHandle, StateRootResult, StateRootTask};
use futures::Stream;
use pin_project::pin_project;
use reth_trie::updates::TrieUpdates;
use revm_primitives::{EvmState, B256};
use std::{
    future::Future,
    pin::Pin,
    sync::mpsc,
    task::{Context, Poll},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

/// Asynchronous implementation of the state root task
#[pin_project]
#[allow(dead_code)]
pub(crate) struct StateRootAsyncTask<Factory> {
    #[pin]
    state_stream: UnboundedReceiverStream<EvmState>,
    config: StateRootConfig<Factory>,
}

impl<Factory> StateRootTask for StateRootAsyncTask<Factory>
where
    Factory: Send + 'static,
{
    type StateStream = UnboundedReceiverStream<EvmState>;
    type Factory = Factory;

    fn new(config: StateRootConfig<Factory>, state_stream: Self::StateStream) -> Self {
        Self { config, state_stream }
    }

    fn spawn(self) -> StateRootHandle {
        let (tx, rx) = mpsc::channel();

        tokio::spawn(async move {
            debug!(target: "engine::tree", "Starting async state root task");
            let result = self.await;
            let _ = tx.send(result);
        });

        StateRootHandle::new(rx)
    }

    fn on_state_update(
        _view: &reth_provider::providers::ConsistentDbView<impl Send + 'static>,
        _input: &std::sync::Arc<reth_trie::TrieInput>,
        _state: EvmState,
    ) {
        // Default implementation of state update handling
        // TODO: calculate hashed state update and dispatch proof gathering for it.
    }
}

impl<Factory> Future for StateRootAsyncTask<Factory>
where
    Factory: Send + 'static,
{
    type Output = StateRootResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        // Process all items until the stream is closed
        loop {
            match this.state_stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(state)) => {
                    // new state received, process it
                    Self::on_state_update(&this.config.consistent_view, &this.config.input, state);
                }
                Poll::Ready(None) => {
                    // stream closed, calculate and return final result
                    return Poll::Ready(Ok((B256::default(), TrieUpdates::default())));
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
        // TODO:
        //    * keep track of proof calculation
        //    * keep track of intermediate root computation
        //    * return final state root result
    }
}
