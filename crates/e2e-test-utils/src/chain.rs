use futures_util::{ready, Future, Stream};
use std::{collections::VecDeque, pin::Pin, task::Poll, time::Duration};
use tokio::time::{sleep_until, Instant, Sleep};

enum Action<T: Unpin, R: Unpin> {
    Next(Box<dyn FnOnce(T) -> R + Send>),
    Wait(Duration),
}

pub struct ChainBuilder<T: Unpin, R: Unpin> {
    actions: VecDeque<Action<T, R>>,
}

impl<T: Unpin, R: Unpin> ChainBuilder<T, R> {
    pub fn new() -> Self {
        ChainBuilder::default()
    }

    pub fn next<F: FnOnce(T) -> R + 'static + Send>(mut self, func: F) -> Self {
        self.actions.push_back(Action::Next(Box::new(func)));
        self
    }

    pub fn wait(mut self, duration: Duration) -> Self {
        self.actions.push_back(Action::Wait(duration));
        self
    }

    pub fn build(self) -> Chain<T, R> {
        Chain { actions: self.actions, sleep: None, result: None }
    }
}

impl<T: Unpin, R: Unpin> Default for ChainBuilder<T, R> {
    fn default() -> Self {
        ChainBuilder { actions: VecDeque::new() }
    }
}

pub struct Chain<T: Unpin, R: Unpin> {
    actions: VecDeque<Action<T, R>>,
    sleep: Option<Pin<Box<Sleep>>>,
    result: Option<T>,
}

impl<T: Unpin, R: Unpin> Chain<T, R> {
    fn next_action(&mut self) -> Option<Action<T, R>> {
        self.actions.pop_front()
    }
}

impl<T: Unpin, R: Unpin> Stream for Chain<T, R> {
    type Item = R;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Try polling the sleep future first
        if let Some(ref mut sleep) = self.sleep {
            ready!(Pin::new(sleep).poll(cx));
            // Since we're ready, discard the sleep future
            self.sleep.take();
        }

        match self.next_action() {
            Some(action) => match action {
                Action::Next(func) => {
                    let result = (func)(self.result.take().unwrap());
                    Poll::Ready(Some(result))
                }
                Action::Wait(duration) => {
                    // Set up a sleep future and schedule this future to be polled again for it.
                    self.sleep = Some(Box::pin(sleep_until(Instant::now() + duration)));
                    cx.waker().wake_by_ref();

                    Poll::Pending
                }
            },
            None => Poll::Ready(None),
        }
    }
}
