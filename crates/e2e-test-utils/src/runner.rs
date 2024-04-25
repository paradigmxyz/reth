use crate::{NodeTestCtx, TmpNodeAdapter};
use futures_util::Stream;
use reth_node_builder::Node;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{sleep_until, Instant, Sleep};

/// A builder for constructing a stream of actions to be executed sequentially.
///
/// This struct manages asynchronous tasks associated with a `NodeTestCtx` context.
/// It allows for queueing actions that mutate the context and wait operations for delaying further
/// actions.
///
/// # Examples
///
/// ```ignore
/// use reth_e2e_test_utils::runner::{ActionRunner, ActionStream};
/// use reth_e2e_test_utils::NodeTestCtx;
/// use reth_node_optimism::OptimismNode;
/// use tokio::time::Duration;
///
/// let action_stream: ActionStream<OptimismNode> = ActionRunner::new(node_test_ctx)
///     .next(|ctx| async move {
///         // Perform operations using `ctx`
///         (ctx, eyre::Result::Ok(()))
///     })
///     .wait(Duration::from_secs(1))
///     .build();
///
/// if let Some(action) = action_stream.next().await {
///     // Process the result of the action
/// }
/// ```
pub struct ActionRunner<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin,
{
    actions: VecDeque<Action<N>>,
    ctx: NodeTestCtx<N>,
}

impl<N> ActionRunner<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin,
{
    /// Creates a new `ActionRunner` with the provided context.
    pub fn new(ctx: NodeTestCtx<N>) -> Self {
        ActionRunner { actions: VecDeque::new(), ctx }
    }

    /// Queues an asynchronous action to be executed with the context.
    ///
    /// The action should take a `NodeTestCtx<N>` and return a future resulting in a tuple of the
    /// context and an `eyre::Result<()>`.
    pub fn next<F, Fut>(mut self, action_fn: F) -> Self
    where
        F: FnOnce(NodeTestCtx<N>) -> Fut + Send + 'static,
        Fut: Future<Output = (NodeTestCtx<N>, eyre::Result<()>)> + Send + 'static,
    {
        let boxed_action = Box::new(move |ctx: NodeTestCtx<N>| -> Pin<Box<ActionRes<N>>> {
            Box::pin(action_fn(ctx))
        });
        self.actions.push_back(Action::Next(boxed_action));
        self
    }

    /// Inserts a delay into the action stream.
    ///
    /// The action stream will pause for the specified duration before proceeding to the next
    /// action.
    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push_back(Action::Wait(duration));
        self
    }

    /// Finalizes the builder and returns a `ActionStream`, ready to be executed.
    pub fn build(self) -> ActionStream<N> {
        ActionStream { actions: self.actions, ctx: Some(self.ctx), future: None, sleep: None }
    }
}

/// Represents an action to be executed in an `ActionStream`.
///
/// Actions can either execute asynchronous operations (`Next`) or impose a delay (`Wait`).
pub enum Action<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin,
{
    Next(Box<ActionFunc<N>>),
    /// A delay action that pauses the action stream for a specified duration.
    Wait(Duration),
}

/// A `ActionStream` manages the execution of a series of `Action`s.
///
/// It implements `Stream` and yields the result of each action, allowing asynchronous tasks to be
/// processed in sequence.
pub struct ActionStream<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin,
{
    actions: VecDeque<Action<N>>,
    future: Option<Pin<Box<ActionFuture<N>>>>,
    sleep: Option<Pin<Box<Sleep>>>,
    ctx: Option<NodeTestCtx<N>>,
}

impl<N> ActionStream<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin + Clone,
{
    /// Advances to the next action, if available.
    fn next_action(&mut self) -> Option<Action<N>> {
        self.actions.pop_front()
    }
}

impl<N> Stream for ActionStream<N>
where
    N: Default + Node<TmpNodeAdapter<N>> + Unpin,
{
    type Item = (Option<NodeTestCtx<N>>, eyre::Result<()>);

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            // Check if there's a sleep future active and attempt to poll it.
            if let Some(sleep) = this.sleep.as_mut() {
                if sleep.as_mut().poll(cx).is_ready() {
                    // If the sleep completes, clear the future to allow next actions to proceed.
                    this.sleep = None;
                } else {
                    // If the sleep has not completed, return `Poll::Pending`.
                    return Poll::Pending;
                }
            }

            // Check if there's a future active and attempt to poll it.
            if let Some(future) = this.future.as_mut() {
                if let Poll::Ready(mut res) = future.as_mut().poll(cx) {
                    // If the future completes, capture the context from the result and clear the
                    // future.
                    this.future = None;
                    let ctx = res.0.take(); // Take the context out of the result.
                    this.ctx = ctx; // Store the context back to the stream for future actions.
                } else {
                    // If the future is not ready, return `Poll::Pending`.
                    return Poll::Pending;
                }
            }

            // Only proceed to fetch and prepare the next action if there is no active future or
            // sleep.
            if this.future.is_none() && this.sleep.is_none() {
                match this.next_action() {
                    Some(Action::Next(action)) => {
                        // If the next action is an operation, take the current context and create a
                        // new future.
                        let ctx = this.ctx.take().expect("Test context should be set.");
                        let next = Box::pin(async move {
                            let (ctx, res) = action(ctx).await;
                            (Some(ctx), res)
                        });
                        // Set the new future as the active future to be polled.
                        this.future = Some(next);
                    }
                    Some(Action::Wait(duration)) => {
                        // If the next action is a wait, set up a new sleep future with the
                        // specified duration.
                        this.sleep = Some(Box::pin(sleep_until(Instant::now() + duration)));
                    }
                    None => {
                        // If there are no more actions to process, return `Poll::Ready(Some(()))`
                        // indicating stream completion.
                        return Poll::Ready(Some((this.ctx.take(), Ok(()))));
                    }
                }
            }
        }
    }
}

type ActionRes<N> = dyn Future<Output = (NodeTestCtx<N>, eyre::Result<()>)> + Send;
type ActionFunc<N> = dyn FnOnce(NodeTestCtx<N>) -> Pin<Box<ActionRes<N>>> + Send + 'static;
type ActionFuture<N> = dyn Future<Output = (Option<NodeTestCtx<N>>, eyre::Result<()>)> + Send;
