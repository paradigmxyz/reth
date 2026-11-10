//! Cloneable MPMC channels for the selected backend.

#[cfg(not(feature = "fuzz"))]
pub use crossbeam_channel::*;
#[cfg(feature = "fuzz")]
pub use simulated::*;

/// Sends a value unless `cancel` receives a message or disconnects first.
///
/// Returns the unsent value on cancellation or receiver disconnection. A
/// completed send wins a race with cancellation; rendezvous sends complete only
/// once their value is consumed.
#[cfg(not(feature = "fuzz"))]
pub fn send_or_cancel<T>(
    sender: &Sender<T>,
    value: T,
    cancel: &Receiver<()>,
) -> Result<(), SendError<T>> {
    crossbeam_channel::select! {
        send(sender, value) -> result => result,
        recv(cancel) -> _ => Err(SendError(value)),
    }
}

#[cfg(feature = "fuzz")]
mod simulated {
    use crate::{
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant},
    };
    use std::{collections::VecDeque, fmt};

    pub use crate::{select, select_biased};
    pub use crossbeam_channel::{
        RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError,
    };

    struct State<T> {
        queue: VecDeque<(u64, T)>,
        capacity: Option<usize>,
        next_ticket: u64,
        senders: usize,
        receivers: usize,
        send_waiters: Vec<thread::Thread>,
        recv_waiters: Vec<thread::Thread>,
        direct_receivers: Vec<thread::ThreadId>,
    }

    /// The sending half of a simulated MPMC channel.
    pub struct Sender<T> {
        state: Arc<Mutex<State<T>>>,
    }
    /// The receiving half of a simulated MPMC channel.
    pub struct Receiver<T> {
        state: Arc<Mutex<State<T>>>,
    }

    impl<T> fmt::Debug for Sender<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Sender").finish_non_exhaustive()
        }
    }
    impl<T> fmt::Debug for Receiver<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Receiver").finish_non_exhaustive()
        }
    }

    /// Creates a channel with capacity `capacity`; zero requires a rendezvous.
    pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
        make(Some(capacity))
    }
    /// Creates a receiver that never becomes ready or disconnected.
    pub fn never<T>() -> Receiver<T> {
        let (sender, receiver) = make(None);
        drop(sender);
        receiver.with_permanent_sender()
    }
    /// Creates a channel without a capacity limit.
    pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
        make(None)
    }

    fn make<T>(capacity: Option<usize>) -> (Sender<T>, Receiver<T>) {
        let state = Arc::new(Mutex::new(State {
            queue: VecDeque::new(),
            capacity,
            next_ticket: 0,
            senders: 1,
            receivers: 1,
            send_waiters: Vec::new(),
            recv_waiters: Vec::new(),
            direct_receivers: Vec::new(),
        }));
        (Sender { state: state.clone() }, Receiver { state })
    }

    fn wake(waiters: Vec<thread::Thread>) {
        for waiter in waiters {
            waiter.unpark();
        }
    }
    fn register(waiters: &mut Vec<thread::Thread>) {
        let current = thread::current();
        if !waiters.iter().any(|waiter| waiter.id() == current.id()) {
            waiters.push(current);
        }
    }

    /// Sends a value unless `cancel` receives a message or disconnects first.
    ///
    /// Cancellation retracts an unconsumed rendezvous value and returns it.
    /// A value already accepted by a receiver is committed even if cancellation
    /// is observed before this function resumes.
    pub fn send_or_cancel<T>(
        sender: &Sender<T>,
        value: T,
        cancel: &Receiver<()>,
    ) -> Result<(), SendError<T>> {
        send_blocking(sender, value, Some(cancel))
    }

    fn send_blocking<T>(
        sender: &Sender<T>,
        value: T,
        cancel: Option<&Receiver<()>>,
    ) -> Result<(), SendError<T>> {
        let _registration = SendListener { sender, id: thread::current().id() };
        let mut value = Some(value);
        let mut ticket = None;
        loop {
            let cancellation = cancel.map(Receiver::listen);
            let cancelled = matches!(
                cancel.map(Receiver::try_recv),
                Some(Ok(()) | Err(TryRecvError::Disconnected))
            );
            let mut state = sender.state.lock().expect("channel state is not poisoned");
            if let Some(sent) = ticket {
                let Some(position) = state.queue.iter().position(|(id, _)| *id == sent) else {
                    return Ok(());
                };
                if cancelled || state.receivers == 0 {
                    let value = state.queue.remove(position).expect("pending rendezvous value").1;
                    let waiters = std::mem::take(&mut state.send_waiters);
                    drop(state);
                    wake(waiters);
                    return Err(SendError(value));
                }
            } else {
                if cancelled || state.receivers == 0 {
                    return Err(SendError(value.take().expect("unsent value")));
                }
                if state.capacity.is_none_or(|cap| state.queue.len() < cap.max(1)) {
                    let id = state.next_ticket;
                    state.next_ticket += 1;
                    state.queue.push_back((id, value.take().expect("unsent value")));
                    let waiters = std::mem::take(&mut state.recv_waiters);
                    if state.capacity != Some(0) {
                        drop(state);
                        wake(waiters);
                        return Ok(());
                    }
                    ticket = Some(id);
                    register(&mut state.send_waiters);
                    drop(state);
                    wake(waiters);
                    thread::park();
                    drop(cancellation);
                    continue;
                }
            }
            register(&mut state.send_waiters);
            drop(state);
            thread::park();
            drop(cancellation);
        }
    }

    struct SendListener<'a, T> {
        sender: &'a Sender<T>,
        id: thread::ThreadId,
    }
    impl<T> Drop for SendListener<'_, T> {
        fn drop(&mut self) {
            self.sender
                .state
                .lock()
                .expect("channel state is not poisoned")
                .send_waiters
                .retain(|thread| thread.id() != self.id);
        }
    }

    impl<T> Sender<T> {
        /// Sends a value, blocking until capacity or a rendezvous is available.
        pub fn send(&self, value: T) -> Result<(), SendError<T>> {
            send_blocking(self, value, None)
        }

        /// Tries to send without blocking, returning the value on failure.
        ///
        /// On simulated rendezvous channels this pairs with a blocked `recv` or
        /// `recv_timeout`, not a receive-selection registration. Selection-only
        /// waiters therefore cause `Full`; use blocking `send` with selection.
        pub fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
            let mut state = self.state.lock().expect("channel state is not poisoned");
            if state.receivers == 0 {
                return Err(TrySendError::Disconnected(value));
            }
            if state.capacity.is_some_and(|cap| {
                state.queue.len() >= cap.max(1) || (cap == 0 && state.direct_receivers.is_empty())
            }) {
                return Err(TrySendError::Full(value));
            }
            let id = state.next_ticket;
            state.next_ticket += 1;
            state.queue.push_back((id, value));
            let waiters = std::mem::take(&mut state.recv_waiters);
            drop(state);
            wake(waiters);
            Ok(())
        }

        /// Returns the number of queued messages (zero for a rendezvous channel).
        pub fn len(&self) -> usize {
            let state = self.state.lock().expect("channel state is not poisoned");
            if state.capacity == Some(0) {
                0
            } else {
                state.queue.len()
            }
        }
        /// Returns whether the channel contains no queued messages.
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
        /// Returns the configured bound, or `None` for an unbounded channel.
        pub fn capacity(&self) -> Option<usize> {
            self.state.lock().expect("channel state is not poisoned").capacity
        }
    }

    impl<T> Receiver<T> {
        fn with_permanent_sender(self) -> Self {
            self.state.lock().expect("channel state is not poisoned").senders = 1;
            self
        }

        /// Receives a value, blocking until one arrives or all senders disconnect.
        pub fn recv(&self) -> Result<T, RecvError> {
            loop {
                let listener = self.listen_receive();
                match self.try_recv() {
                    Ok(value) => return Ok(value),
                    Err(TryRecvError::Disconnected) => return Err(RecvError),
                    Err(TryRecvError::Empty) => thread::park(),
                }
                drop(listener);
            }
        }

        /// Receives a value, waiting at most `timeout` from the current virtual time.
        ///
        /// Available values are returned before checking disconnection or the deadline.
        /// Returns [`RecvTimeoutError::Disconnected`] when the queue is empty and all senders
        /// are gone, or [`RecvTimeoutError::Timeout`] when an empty connected channel reaches
        /// the deadline.
        pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
            let deadline = Instant::now() + timeout;
            let timer = crate::deterministic::Timer::new(deadline);
            let result = loop {
                let listener = self.listen_receive();
                match self.try_recv() {
                    Ok(value) => break Ok(value),
                    Err(TryRecvError::Disconnected) => break Err(RecvTimeoutError::Disconnected),
                    Err(TryRecvError::Empty) if Instant::now() >= deadline => {
                        break Err(RecvTimeoutError::Timeout);
                    }
                    Err(TryRecvError::Empty) => thread::park(),
                }
                drop(listener);
            };
            drop(timer);
            result
        }

        /// Tries to receive without blocking.
        pub fn try_recv(&self) -> Result<T, TryRecvError> {
            let mut state = self.state.lock().expect("channel state is not poisoned");
            if let Some((_, value)) = state.queue.pop_front() {
                let waiters = std::mem::take(&mut state.send_waiters);
                drop(state);
                wake(waiters);
                Ok(value)
            } else if state.senders == 0 {
                Err(TryRecvError::Disconnected)
            } else {
                Err(TryRecvError::Empty)
            }
        }

        /// Returns an iterator that blocks until all senders disconnect.
        pub const fn iter(&self) -> Iter<'_, T> {
            Iter(self)
        }
        /// Returns an iterator over messages immediately available.
        pub const fn try_iter(&self) -> TryIter<'_, T> {
            TryIter(self)
        }
        /// Returns the number of queued messages (zero for a rendezvous channel).
        pub fn len(&self) -> usize {
            let state = self.state.lock().expect("channel state is not poisoned");
            if state.capacity == Some(0) {
                0
            } else {
                state.queue.len()
            }
        }
        /// Returns whether the channel contains no queued messages.
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
        /// Returns the configured bound, or `None` for an unbounded channel.
        pub fn capacity(&self) -> Option<usize> {
            self.state.lock().expect("channel state is not poisoned").capacity
        }

        fn listen_receive(&self) -> Listener<'_, T> {
            let listener = self.listen();
            self.state
                .lock()
                .expect("channel state is not poisoned")
                .direct_receivers
                .push(listener.id);
            listener
        }

        /// Creates a receiver-typed result slot for selection macro expansion.
        #[doc(hidden)]
        pub const fn selection_slot(&self) -> Option<Result<T, RecvError>> {
            None
        }

        /// Registers the current task for receive readiness; used by selection macros.
        #[doc(hidden)]
        pub fn listen(&self) -> Listener<'_, T> {
            let mut state = self.state.lock().expect("channel state is not poisoned");
            register(&mut state.recv_waiters);
            Listener { receiver: self, id: thread::current().id() }
        }
    }

    /// A receive-readiness registration removed when dropped.
    #[doc(hidden)]
    #[derive(Debug)]
    pub struct Listener<'a, T> {
        receiver: &'a Receiver<T>,
        id: thread::ThreadId,
    }
    impl<T> Drop for Listener<'_, T> {
        fn drop(&mut self) {
            let mut state = self.receiver.state.lock().expect("channel state is not poisoned");
            state.recv_waiters.retain(|thread| thread.id() != self.id);
            state.direct_receivers.retain(|id| *id != self.id);
        }
    }

    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Self {
            self.state.lock().expect("channel state is not poisoned").senders += 1;
            Self { state: self.state.clone() }
        }
    }
    impl<T> Clone for Receiver<T> {
        fn clone(&self) -> Self {
            self.state.lock().expect("channel state is not poisoned").receivers += 1;
            Self { state: self.state.clone() }
        }
    }
    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            let mut state = self.state.lock().expect("channel state is not poisoned");
            state.senders -= 1;
            let waiters = if state.senders == 0 {
                std::mem::take(&mut state.recv_waiters)
            } else {
                Vec::new()
            };
            drop(state);
            wake(waiters);
        }
    }
    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            let mut state = self.state.lock().expect("channel state is not poisoned");
            state.receivers -= 1;
            let waiters = if state.receivers == 0 {
                std::mem::take(&mut state.send_waiters)
            } else {
                Vec::new()
            };
            drop(state);
            wake(waiters);
        }
    }

    /// A blocking iterator borrowing a receiver.
    #[derive(Debug)]
    pub struct Iter<'a, T>(&'a Receiver<T>);
    impl<T> Iterator for Iter<'_, T> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            self.0.recv().ok()
        }
    }
    /// An iterator over immediately available messages.
    #[derive(Debug)]
    pub struct TryIter<'a, T>(&'a Receiver<T>);
    impl<T> Iterator for TryIter<'_, T> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            self.0.try_recv().ok()
        }
    }
    /// A blocking iterator owning its receiver.
    #[derive(Debug)]
    pub struct IntoIter<T>(Receiver<T>);
    impl<T> Iterator for IntoIter<T> {
        type Item = T;
        fn next(&mut self) -> Option<T> {
            self.0.recv().ok()
        }
    }
    impl<T> IntoIterator for Receiver<T> {
        type Item = T;
        type IntoIter = IntoIter<T>;
        fn into_iter(self) -> Self::IntoIter {
            IntoIter(self)
        }
    }
    impl<'a, T> IntoIterator for &'a Receiver<T> {
        type Item = T;
        type IntoIter = Iter<'a, T>;
        fn into_iter(self) -> Self::IntoIter {
            self.iter()
        }
    }
}

/// Selects a ready receive operation in declaration order in simulation.
///
/// Like the native counterpart, this prioritizes earlier ready branches. Simulation supports
/// one to eight receive branches and an optional immediate default, not send branches or timed
/// defaults.
#[cfg(feature = "fuzz")]
#[macro_export]
macro_rules! select_biased {
    ($(recv($receiver:expr) -> $binding:pat => $body:expr),+ $(,)?) => {{
        let receivers = ($($crate::channel::Receiver::clone(&$receiver),)+);
        $crate::__select_recv!(@index receivers; []; []; [0 1 2 3 4 5 6 7]; $(($binding, $body)),+)
    }};
    ($(recv($receiver:expr) -> $binding:pat => $body:expr,)+ default => $default:expr $(,)?) => {{
        let receivers = ($($crate::channel::Receiver::clone(&$receiver),)+);
        $crate::__select_recv!(@index receivers; [$default]; []; [0 1 2 3 4 5 6 7]; $(($binding, $body)),+)
    }};
}

/// Selects a receive operation; simulation uses fixed declaration-order priority.
///
/// Unlike native selection, simulation does not randomize ready branches. Its supported syntax
/// and eight-receive-branch limit are the same as [`select_biased!`].
#[cfg(feature = "fuzz")]
#[macro_export]
macro_rules! select { ($($tokens:tt)*) => { $crate::select_biased! { $($tokens)* } }; }

/// Internal receive-selection expansion.
#[doc(hidden)]
#[cfg(feature = "fuzz")]
#[macro_export]
macro_rules! __select_recv {
    (@index $receivers:ident; [$($default:expr)?]; [$($done:tt)*]; [$index:tt $($indices:tt)*]; ($binding:pat, $body:expr), $($rest:tt)*) => {
        $crate::__select_recv!(@index $receivers; [$($default)?]; [$($done)* ($index, $binding, $body)]; [$($indices)*]; $($rest)*)
    };
    (@index $receivers:ident; [$($default:expr)?]; [$($done:tt)*]; [$index:tt $($indices:tt)*]; ($binding:pat, $body:expr)) => {
        $crate::__select_recv!(@run $receivers; [$($default)?]; $($done)* ($index, $binding, $body))
    };
    (@run $receivers:ident; [$($default:expr)?]; $(($index:tt, $binding:pat, $body:expr))*) => {{
        let mut results = ($($receivers.$index.selection_slot(),)*);
        let selected = loop {
            let listeners = ($($receivers.$index.listen(),)*);
            $(match $receivers.$index.try_recv() {
                Err($crate::channel::TryRecvError::Empty) => {}
                result => {
                    results.$index = Some(result.map_err(|_| $crate::channel::RecvError));
                    drop(listeners);
                    break Some($index);
                }
            })*
            $crate::__select_recv!(@wait listeners; [$($default)?]);
        };
        drop($receivers);
        match selected {
            $(Some($index) => {
                let $binding = results.$index.take().expect("selected receive has a result");
                $body
            },)*
            None => $crate::__select_recv!(@default [$($default)?]),
            Some(_) => unreachable!("only declared receive branches can be selected"),
        }
    }};
    (@wait $listeners:ident; []) => { $crate::thread::park(); drop($listeners); };
    (@wait $listeners:ident; [$default:expr]) => { drop($listeners); break None; };
    (@default []) => { unreachable!("blocking selection always selects a receive") };
    (@default [$default:expr]) => { $default };
}

/// Standard-library-style MPSC channels selected with the executor backend.
pub mod mpsc {
    #[cfg(feature = "fuzz")]
    pub use super::{
        IntoIter, Iter, Receiver, RecvError, RecvTimeoutError, SendError, Sender, TryIter,
        TryRecvError, TrySendError,
    };
    #[cfg(not(feature = "fuzz"))]
    pub use std::sync::mpsc::*;
    /// The bounded sending half of a simulated channel.
    #[cfg(feature = "fuzz")]
    pub type SyncSender<T> = super::Sender<T>;
    /// Creates an unbounded simulated MPSC channel.
    #[cfg(feature = "fuzz")]
    pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
        super::unbounded()
    }
    /// Creates a bounded simulated MPSC channel, or a rendezvous when capacity is zero.
    #[cfg(feature = "fuzz")]
    pub fn sync_channel<T>(capacity: usize) -> (SyncSender<T>, Receiver<T>) {
        super::bounded(capacity)
    }
}
