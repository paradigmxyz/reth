use crate::node::NodeTestContext;
use futures_util::{ready, Future, Stream};
use reth_node_builder::FullNodeComponents;
use std::{
    collections::VecDeque,
    marker::PhantomData,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{sleep_until, Instant, Sleep};

pub trait ActionT {
    type Ctx;
    type Output: Unpin;
    type Future: Future<Output = Self::Output> + Send;

    fn execute(&mut self, ctx: &mut Self::Ctx) -> Self::Future;
}

pub struct ActionFn<F, N> {
    func: F,
    _marker: PhantomData<N>,
}

impl<F, Fut, N> ActionT for ActionFn<F, N>
where
    N: FullNodeComponents,
    F: FnOnce(&mut NodeTestContext<N>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<()>> + Send,
{
    type Ctx = NodeTestContext<N>;
    type Output = eyre::Result<()>;
    type Future = Fut;

    fn execute(&mut self, ctx: &mut Self::Ctx) -> Pin<Box<Self::Future>> {
        Box::pin((self.func)(ctx))
    }
}

enum Action<A: ActionT> {
    Next(Box<dyn FnOnce(Option<A::Output>) -> Pin<Box<A>> + Send + 'static>),
    Wait(Duration),
}
pub struct ChainRunner<A: ActionT> {
    actions: VecDeque<Action<A>>,
    ctx: A::Ctx,
}

impl<A: ActionT> ChainRunner<A> {
    pub fn new(ctx: A::Ctx) -> Self {
        ChainRunner { actions: VecDeque::new(), ctx }
    }

    pub fn next(
        mut self,
        action_fn: impl FnOnce(Option<A::Output>) -> Pin<Box<A>> + Send + 'static,
    ) -> Self {
        self.actions.push_back(Action::Next(Box::new(action_fn)));
        self
    }

    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push_back(Action::Wait(duration));
        self
    }

    pub fn build(self) -> Chain<A> {
        Chain {
            actions: self.actions,
            ctx: self.ctx,
            sleep: None,
            future: None,
            previous_item: None,
        }
    }
}

pub struct Chain<A: ActionT> {
    actions: VecDeque<Action<A>>,
    ctx: A::Ctx,
    future: Option<Pin<Box<A::Future>>>,
    previous_item: Option<A::Output>,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<A: ActionT> Chain<A> {
    fn next_action(&mut self) -> Option<Action<A>> {
        self.actions.pop_front()
    }
}

impl<A: ActionT> Stream for Chain<A>
where
    A::Output: Unpin,
    <A as ActionT>::Ctx: Unpin,
{
    type Item = A::Output;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Try polling the future next
        if let Some(ref mut future) = this.future {
            let result = ready!(future.as_mut().poll(cx));
            // Store the result in previous_item before discarding the future
            this.previous_item = Some(result);
            // Since we're ready, discard the future
            this.future.take();
            return Poll::Ready(this.previous_item.take());
        }

        match this.next_action() {
            Some(action) => match action {
                Action::Next(action_fn) => {
                    // Execute the action and store the future
                    let mut action = action_fn(this.previous_item.take());
                    this.future = Some(Box::pin(action.execute(&mut this.ctx)));
                    cx.waker().wake_by_ref();

                    Poll::Pending
                }
                Action::Wait(duration) => {
                    // Set up a sleep future and schedule this future to be polled again for it.
                    this.sleep = Some(Box::pin(sleep_until(Instant::now() + duration)));
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            },
            None => Poll::Ready(None),
        }
    }
}
