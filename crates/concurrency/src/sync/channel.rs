//! Cloneable MPMC channels for the selected backend.

#[cfg(not(feature = "fuzz"))]
pub use crossbeam_channel::*;
#[cfg(feature = "fuzz")]
pub use simulated::*;

/// Selects between sending a value and receiving a cancellation message or disconnection.
///
/// Returns the unsent value when cancellation is selected or the output disconnects.
/// Selection is unbiased: if both operations are ready, either may win, even when
/// cancellation was already signalled. A rendezvous send completes only when consumed.
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
    use crate::sync::{
        deterministic::{rendezvous_selection, Timer, Waiter},
        thread,
        time::{Duration, Instant},
        Arc, Mutex,
    };
    use std::{collections::VecDeque, fmt, mem};

    pub use crate::{select, select_biased};
    pub use crossbeam_channel::{
        RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError,
    };

    /// The sending half of a simulated MPMC channel.
    pub struct Sender<T> {
        state: Arc<Mutex<State<T>>>,
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
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.receivers == 0 {
                return Err(TrySendError::Disconnected(value));
            }
            if state.capacity == Some(0) {
                let Some((_, slot)) =
                    state.direct_receivers.iter_mut().find(|(_, slot)| slot.is_none())
                else {
                    return Err(TrySendError::Full(value));
                };
                // Claim a particular receiver; the value can no longer time out or be stolen.
                *slot = Some(value);
                let waiters = mem::take(&mut state.recv_waiters);
                drop(state);
                wake(waiters);
                return Ok(());
            }
            if state.capacity.is_some_and(|cap| state.queue.len() >= cap) {
                return Err(TrySendError::Full(value));
            }
            let id = state.next_ticket;
            state.next_ticket += 1;
            state.queue.push_back((id, value));
            let waiters = mem::take(&mut state.recv_waiters);
            drop(state);
            wake(waiters);
            Ok(())
        }

        /// Returns the number of queued messages (zero for a rendezvous channel).
        pub fn len(&self) -> usize {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
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
            self.state.lock().unwrap_or_else(|error| error.into_inner()).capacity
        }
    }

    impl<T> fmt::Debug for Sender<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Sender").finish_non_exhaustive()
        }
    }

    impl<T> Clone for Sender<T> {
        fn clone(&self) -> Self {
            self.state.lock().unwrap_or_else(|error| error.into_inner()).senders += 1;
            Self { state: self.state.clone() }
        }
    }

    impl<T> Drop for Sender<T> {
        fn drop(&mut self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.senders -= 1;
            let waiters =
                if state.senders == 0 { mem::take(&mut state.recv_waiters) } else { Vec::new() };
            drop(state);
            wake(waiters);
        }
    }

    /// The receiving half of a simulated MPMC channel.
    pub struct Receiver<T> {
        state: Arc<Mutex<State<T>>>,
    }

    impl<T> Receiver<T> {
        fn with_permanent_sender(self) -> Self {
            self.state.lock().unwrap_or_else(|error| error.into_inner()).senders = 1;
            self
        }

        /// Receives a value, blocking until one arrives or all senders disconnect.
        pub fn recv(&self) -> Result<T, RecvError> {
            self.recv_deadline(None).map_err(|_| RecvError)
        }

        /// Receives a value, waiting at most `timeout` from the current virtual time.
        ///
        /// Available values are returned before checking disconnection or the deadline.
        /// An unrepresentable deadline waits without a time limit, like the native backend.
        /// Returns [`RecvTimeoutError::Disconnected`] when the queue is empty and all senders
        /// are gone, or [`RecvTimeoutError::Timeout`] when an empty connected channel reaches
        /// the deadline.
        pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
            self.recv_deadline(Instant::now().checked_add(timeout))
        }

        fn recv_deadline(&self, deadline: Option<Instant>) -> Result<T, RecvTimeoutError> {
            let listener = self.listen();
            let timer = deadline.map(|deadline| Timer::new(deadline, listener.waiter.clone()));
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.direct_receivers.push((listener.id, None));
            loop {
                let slot = state
                    .direct_receivers
                    .iter_mut()
                    .find(|(id, _)| *id == listener.id)
                    .expect("active receive registration");
                let result = if let Some(value) = slot.1.take() {
                    Some(Ok(value))
                } else if let Some((_, value)) = state.queue.pop_front() {
                    Some(Ok(value))
                } else if state.senders == 0 {
                    Some(Err(RecvTimeoutError::Disconnected))
                } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    Some(Err(RecvTimeoutError::Timeout))
                } else {
                    None
                };
                if let Some(result) = result {
                    // Removing readiness under the same lock as the timeout decision prevents
                    // a successful try_send from targeting a receiver that already timed out.
                    state.direct_receivers.retain(|(id, _)| *id != listener.id);
                    let waiters = mem::take(&mut state.send_waiters);
                    drop(state);
                    wake(waiters);
                    drop(timer);
                    return result;
                }
                register(&mut state.recv_waiters, &listener.waiter);
                drop(state);
                listener.wait();
                state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            }
        }

        /// Tries to receive without blocking.
        pub fn try_recv(&self) -> Result<T, TryRecvError> {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some((_, value)) = state.queue.pop_front() {
                let waiters = mem::take(&mut state.send_waiters);
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
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
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
            self.state.lock().unwrap_or_else(|error| error.into_inner()).capacity
        }

        /// Creates a receiver-typed result slot for selection macro expansion.
        #[doc(hidden)]
        pub const fn selection_slot(&self) -> Option<Result<T, RecvError>> {
            None
        }

        /// Registers the current task for receive readiness; used by selection macros.
        #[doc(hidden)]
        pub fn listen(&self) -> Listener<'_, T> {
            self.listen_waiter(Waiter::new())
        }

        /// Shares a selection's private notification token across receive branches.
        #[doc(hidden)]
        pub fn listen_with<U>(&self, listener: &Listener<'_, U>) -> Listener<'_, T> {
            self.listen_waiter(listener.waiter.clone())
        }

        fn listen_waiter(&self, waiter: Waiter) -> Listener<'_, T> {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            register(&mut state.recv_waiters, &waiter);
            Listener { receiver: self, id: waiter.id(), waiter }
        }
    }

    impl<T> fmt::Debug for Receiver<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Receiver").finish_non_exhaustive()
        }
    }

    impl<T> Clone for Receiver<T> {
        fn clone(&self) -> Self {
            self.state.lock().unwrap_or_else(|error| error.into_inner()).receivers += 1;
            Self { state: self.state.clone() }
        }
    }

    impl<T> Drop for Receiver<T> {
        fn drop(&mut self) {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.receivers -= 1;
            let waiters =
                if state.receivers == 0 { mem::take(&mut state.send_waiters) } else { Vec::new() };
            // Rendezvous offers can still belong to blocked senders, which recover them on
            // disconnection. Buffered values have already been sent and must be released now.
            let abandoned = if state.receivers == 0 && state.capacity != Some(0) {
                mem::take(&mut state.queue)
            } else {
                VecDeque::new()
            };
            drop(state);
            wake(waiters);
            // Payload destructors can re-enter this channel or drop another channel's receiver.
            drop(abandoned);
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

    /// A receive-readiness registration removed when dropped.
    #[doc(hidden)]
    #[derive(Debug)]
    pub struct Listener<'a, T> {
        receiver: &'a Receiver<T>,
        id: thread::ThreadId,
        waiter: Waiter,
    }
    impl<T> Listener<'_, T> {
        /// Blocks on this selection's private notification token.
        #[doc(hidden)]
        pub fn wait(&self) {
            self.waiter.wait();
        }
    }
    impl<T> Drop for Listener<'_, T> {
        fn drop(&mut self) {
            let mut state = self.receiver.state.lock().unwrap_or_else(|error| error.into_inner());
            state.recv_waiters.retain(|thread| thread.id() != self.id);
            let abandoned = state
                .direct_receivers
                .iter()
                .position(|(id, _)| *id == self.id)
                .map(|position| state.direct_receivers.remove(position));
            drop(state);
            drop(abandoned);
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

    // Payload callbacks never run while this private state is locked. Shuttle can mark
    // it poisoned when an unrelated logical task unwinds on the same OS thread; all
    // internal accesses recover the intact state instead of propagating that poison.
    struct State<T> {
        queue: VecDeque<(u64, T)>,
        capacity: Option<usize>,
        next_ticket: u64,
        senders: usize,
        receivers: usize,
        send_waiters: Vec<Waiter>,
        recv_waiters: Vec<Waiter>,
        direct_receivers: Vec<(thread::ThreadId, Option<T>)>,
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
                .unwrap_or_else(|error| error.into_inner())
                .send_waiters
                .retain(|thread| thread.id() != self.id);
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

    /// Sends a value unless `cancel` receives a message or disconnects first.
    ///
    /// Simulation checks cancellation before offering a value; native selection is unbiased
    /// when both operations are ready. Cancellation retracts an unconsumed rendezvous value
    /// and returns it. Pending rendezvous selection decisions are serialized across channels.
    /// A value already accepted by a receiver is committed even if cancellation
    /// is observed before this function resumes.
    pub fn send_or_cancel<T>(
        sender: &Sender<T>,
        value: T,
        cancel: &Receiver<()>,
    ) -> Result<(), SendError<T>> {
        send_blocking(sender, value, Some(cancel))
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

    fn wake(waiters: Vec<Waiter>) {
        for waiter in waiters {
            waiter.notify();
        }
    }
    fn register(waiters: &mut Vec<Waiter>, current: &Waiter) {
        if !waiters.iter().any(|waiter| waiter.id() == current.id()) {
            waiters.push(current.clone());
        }
    }

    fn send_blocking<T>(
        sender: &Sender<T>,
        value: T,
        cancel: Option<&Receiver<()>>,
    ) -> Result<(), SendError<T>> {
        let waiter = Waiter::new();
        let _registration = SendListener { sender, id: waiter.id() };
        let mut value = Some(value);
        let mut ticket = None;
        loop {
            let cancellation = cancel.map(|cancel| cancel.listen_waiter(waiter.clone()));
            // Before offering a value there is no send commitment to protect.
            let mut cancelled = ticket.is_none() &&
                matches!(
                    cancel.map(Receiver::try_recv),
                    Some(Ok(()) | Err(TryRecvError::Disconnected))
                );
            let mut cancelled_offer = None;
            // Serialize two-channel decisions so reciprocal selections cannot each hold their
            // output lock while waiting for the other's cancellation lock.
            let selection_lock = (ticket.is_some() && cancel.is_some()).then(rendezvous_selection);
            let selection = selection_lock
                .as_ref()
                .map(|lock| lock.lock().unwrap_or_else(|error| error.into_inner()));
            let mut state = sender.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(sent) = ticket {
                let Some(mut position) = state.queue.iter().position(|(id, _)| *id == sent) else {
                    return Ok(());
                };
                if let Some(cancel) = cancel {
                    // An aliased cancellation receive can claim another sender's rendezvous
                    // offer, but never its own. Keep the removed payload until after unlocking.
                    if Arc::as_ptr(&sender.state).cast::<()>() ==
                        Arc::as_ptr(&cancel.state).cast::<()>()
                    {
                        if let Some(other) = state.queue.iter().position(|(id, _)| *id != sent) {
                            cancelled_offer = state.queue.remove(other);
                            position -= usize::from(other < position);
                            cancelled = true;
                        }
                    } else {
                        // Keep acceptance and cancellation atomic under both channel locks.
                        let mut cancellation =
                            cancel.state.lock().unwrap_or_else(|error| error.into_inner());
                        cancelled =
                            cancellation.queue.pop_front().is_some() || cancellation.senders == 0;
                        let waiters = if cancelled {
                            mem::take(&mut cancellation.send_waiters)
                        } else {
                            Vec::new()
                        };
                        drop(cancellation);
                        wake(waiters);
                    }
                }
                if cancelled || state.receivers == 0 {
                    let value = state.queue.remove(position).expect("pending rendezvous value").1;
                    let waiters = mem::take(&mut state.send_waiters);
                    drop(state);
                    drop(cancelled_offer);
                    wake(waiters);
                    return Err(SendError(value));
                }
            } else {
                if cancelled || state.receivers == 0 {
                    return Err(SendError(value.take().expect("unsent value")));
                }
                // Each blocked rendezvous sender needs an offer of its own, so a selecting
                // receiver can skip its own offer and still pair with another sender.
                if state.capacity.is_none_or(|cap| cap == 0 || state.queue.len() < cap) {
                    let id = state.next_ticket;
                    state.next_ticket += 1;
                    state.queue.push_back((id, value.take().expect("unsent value")));
                    let waiters = mem::take(&mut state.recv_waiters);
                    if state.capacity != Some(0) {
                        drop(state);
                        wake(waiters);
                        return Ok(());
                    }
                    ticket = Some(id);
                    register(&mut state.send_waiters, &waiter);
                    drop(state);
                    wake(waiters);
                    drop(selection);
                    waiter.wait();
                    drop(cancellation);
                    continue;
                }
            }
            register(&mut state.send_waiters, &waiter);
            drop(state);
            drop(selection);
            waiter.wait();
            drop(cancellation);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::sync::Runtime;

        #[test]
        fn receiver_drop_returns_queued_rendezvous_offer_to_sender() {
            Runtime::new().run(|| {
                let (producer, receiver) = bounded(0);
                let sender = producer.clone();
                let task = thread::spawn(move || sender.send(vec![1, 2, 3]));
                // Observe the queued offer before disconnecting, not merely the sender starting.
                while producer.state.lock().unwrap().queue.is_empty() {
                    thread::yield_now();
                }
                drop(receiver);
                assert_eq!(task.join().unwrap(), Err(SendError(vec![1, 2, 3])));
                assert!(producer.state.lock().unwrap().queue.is_empty());
            });
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
        let receivers: ($($crate::__select_recv!(@receiver_type $receiver),)+) = ($(&$receiver,)+);
        $crate::__select_recv!(@index receivers; []; []; [0 1 2 3 4 5 6 7]; $(($binding, $body)),+)
    }};
    ($(recv($receiver:expr) -> $binding:pat => $body:expr,)+ default => $default:expr $(,)?) => {{
        let receivers: ($($crate::__select_recv!(@receiver_type $receiver),)+) = ($(&$receiver,)+);
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
    (@receiver_type $receiver:expr) => { &$crate::sync::channel::Receiver<_> };
    (@index $receivers:ident; [$($default:expr)?]; [$($done:tt)*]; [$index:tt $($indices:tt)*]; ($binding:pat, $body:expr), $($rest:tt)*) => {
        $crate::__select_recv!(@index $receivers; [$($default)?]; [$($done)* ($index, $binding, $body)]; [$($indices)*]; $($rest)*)
    };
    (@index $receivers:ident; [$($default:expr)?]; [$($done:tt)*]; [$index:tt $($indices:tt)*]; ($binding:pat, $body:expr)) => {
        $crate::__select_recv!(@run $receivers; [$($default)?]; $($done)* ($index, $binding, $body))
    };
    (@run $receivers:ident; [$($default:expr)?]; $(($index:tt, $binding:pat, $body:expr))*) => {{
        let mut results = ($($receivers.$index.selection_slot(),)*);
        let listener = $receivers.0.listen();
        let selected = loop {
            let listeners = ($($receivers.$index.listen_with(&listener),)*);
            $(match $receivers.$index.try_recv() {
                Err($crate::sync::channel::TryRecvError::Empty) => {}
                result => {
                    results.$index = Some(result.map_err(|_| $crate::sync::channel::RecvError));
                    drop(listeners);
                    break Some($index);
                }
            })*
            $crate::__select_recv!(@wait listener, listeners; [$($default)?]);
        };
        drop(listener);
        match selected {
            $(Some($index) => {
                let $binding = results.$index.take().expect("selected receive has a result");
                $body
            },)*
            None => $crate::__select_recv!(@default [$($default)?]),
            Some(_) => unreachable!("only declared receive branches can be selected"),
        }
    }};
    (@wait $listener:ident, $listeners:ident; []) => { $listener.wait(); drop($listeners); };
    (@wait $listener:ident, $listeners:ident; [$default:expr]) => { drop($listeners); break None; };
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
