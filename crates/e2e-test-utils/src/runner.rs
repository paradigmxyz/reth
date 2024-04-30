use crate::node::NodeTestContext;
use futures_util::{Future, Stream};
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
    type Event: Unpin;
    type Future: Future<Output = Self::Event> + Send;

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
    type Event = eyre::Result<()>;
    type Future = Fut;

    fn execute(&mut self, ctx: &mut Self::Ctx) -> Self::Future {
        (self.func)(ctx)
    }
}

enum Action<A: ActionT> {
    Next(A::Future),
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

    pub fn next(mut self, action: impl FnOnce() -> A::Future + Send + 'static) -> Self {
        let future = action();
        self.actions.push_back(Action::Next(future));
        self
    }

    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push_back(Action::Wait(duration));
        self
    }

    pub fn build(self) -> Chain<A> {
        Chain { actions: self.actions, ctx: self.ctx, sleep: None, future: None }
    }
}

pub struct Chain<A: ActionT> {
    actions: VecDeque<Action<A>>,
    ctx: A::Ctx,
    future: Option<Pin<Box<A::Future>>>,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<A: ActionT> Chain<A> {
    fn next_action(&mut self) -> Option<Action<A>> {
        self.actions.pop_front()
    }
}

impl<A: ActionT> Stream for Chain<A> {
    type Item = A::Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            // First, try to progress any existing futures.
            if let Some(future) = this.future.as_mut() {
                return match future.as_mut().poll(cx) {
                    Poll::Ready(event) => {
                        this.future = None; // Clear the future once it's done
                        Poll::Ready(Some(event)) // Return the event
                    }
                    Poll::Pending => Poll::Pending,
                };
            }

            // Next, check if there's a sleep that needs to poll.
            if let Some(sleep) = this.sleep.as_mut() {
                return match sleep.as_mut().poll(cx) {
                    Poll::Ready(_) => {
                        this.sleep = None; // Clear the sleep once it's done
                        self.poll_next(cx) // Immediately try to continue processing
                    }
                    Poll::Pending => Poll::Pending,
                };
            }

            // Handle the next action if no futures or sleeps are active.
            match this.next_action() {
                Some(Action::Next(future)) => {
                    this.future = Some(Box::pin(future));
                    return Poll::Pending
                }
                Some(Action::Wait(duration)) => {
                    this.sleep = Some(Box::pin(sleep_until(Instant::now() + duration)));
                    return Poll::Pending
                }
                None => return Poll::Ready(None),
            }
        }
    }
}
