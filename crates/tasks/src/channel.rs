//! Centralized channel type aliases backed by [`crossbeam_channel`].

pub use crossbeam_channel::{
    bounded, select, select_biased, unbounded, Receiver, RecvError, RecvTimeoutError, SendError,
    Sender, TryRecvError, TrySendError,
};

/// Creates a bounded channel with capacity 1, usable as a oneshot.
pub fn oneshot<T>() -> (Sender<T>, Receiver<T>) {
    bounded(1)
}
