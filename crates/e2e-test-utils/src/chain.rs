use futures_util::{ready, Future, Stream};
use std::{collections::VecDeque, pin::Pin, task::Poll, time::Duration};
use tokio::time::{sleep_until, Instant, Sleep};

enum Action<F: Future + Send> {
    Next(Box<dyn FnOnce(Option<F::Output>) -> Pin<Box<F>> + Send + 'static>),
    Wait(Duration),
}

pub struct ChainBuilder<F: Future + Send> {
    actions: VecDeque<Action<F>>,
}

impl<F: Future + Send> ChainBuilder<F> {
    pub fn new() -> Self {
        ChainBuilder::default()
    }

    pub fn next(
        mut self,
        future_fn: impl FnOnce(Option<F::Output>) -> Pin<Box<F>> + Send + 'static,
    ) -> Self {
        self.actions.push_back(Action::Next(Box::new(future_fn)));
        self
    }

    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push_back(Action::Wait(duration));
        self
    }

    pub fn build(self) -> Chain<F> {
        Chain { actions: self.actions, sleep: None, future: None, previous_item: None }
    }
}

impl<F: Future + Send> Default for ChainBuilder<F> {
    fn default() -> Self {
        ChainBuilder { actions: VecDeque::new() }
    }
}

pub struct Chain<F: Future + Send> {
    actions: VecDeque<Action<F>>,
    sleep: Option<Pin<Box<Sleep>>>,
    future: Option<Pin<Box<F>>>,
    previous_item: Option<F::Output>,
}

impl<F: Future + Send> Chain<F> {
    fn next_action(&mut self) -> Option<Action<F>> {
        self.actions.pop_front()
    }
}

impl<F: Future + Send> Stream for Chain<F>
where
    F: Future + Send,
    F::Output: Unpin,
{
    type Item = F::Output;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Try polling the sleep future first
        if let Some(ref mut sleep) = this.sleep {
            ready!(Pin::new(sleep).poll(cx));
            // Since we're ready, discard the sleep future
            this.sleep.take();
        }

        // Try polling the future next
        if let Some(ref mut future) = this.future {
            let result = ready!(Pin::new(future).poll(cx));
            // Store the result in previous_item before discarding the future
            this.previous_item = Some(result);
            // Since we're ready, discard the future
            this.future.take();
            return Poll::Ready(None);
        }

        match this.next_action() {
            Some(action) => match action {
                Action::Next(future_fn) => {
                    // Store the future and schedule this future to be polled again for it.
                    this.future = Some(future_fn(this.previous_item.take()));
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
