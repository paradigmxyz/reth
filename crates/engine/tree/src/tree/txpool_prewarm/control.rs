use crate::tree::TxPoolPrewarmCacheSnapshot as Snapshot;
use alloy_primitives::B256;
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::RwLock;
use std::{
    fmt::Debug,
    sync::{Arc, Weak},
};

/// Sending side of the txpool prewarming worker's control channel.
pub(super) struct Control<J> {
    commands: Sender<Command<J>>,
    publication: Publication,
}

impl<J> Debug for Control<J> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Control")
            .field(
                "published",
                &self.publication.read().as_ref().map(|snapshot| snapshot.parent_hash()),
            )
            .finish_non_exhaustive()
    }
}

impl<J> Control<J> {
    pub(super) fn new() -> (Arc<Self>, Receiver<Command<J>>) {
        let (commands, receiver) = unbounded();
        let publication = Arc::new(RwLock::new(None));
        (Arc::new(Self { commands, publication }), receiver)
    }

    pub(super) fn publication(&self) -> Publication {
        Arc::clone(&self.publication)
    }

    pub(super) fn start(&self, parent_hash: B256, job: J) {
        let _ = self.commands.send(Command::Start { parent_hash, job });
    }

    pub(super) fn pause(self: &Arc<Self>) -> PauseGuard<J> {
        // Fire-and-forget: the worker never interrupts a transaction mid-execution, so waiting
        // for it to observe the pause would only add that execution time to the caller's
        // latency without reducing the overlap.
        let _ = self.commands.send(Command::Pause);
        PauseGuard { control: Arc::downgrade(self) }
    }

    pub(super) fn snapshot(&self, parent_hash: B256) -> Option<Snapshot> {
        self.publication
            .read()
            .as_ref()
            .filter(|snapshot| snapshot.parent_hash() == parent_hash)
            .cloned()
    }
}

/// Immutable snapshot publication shared by the handle and worker.
pub(super) type Publication = Arc<RwLock<Option<Snapshot>>>;

/// Commands sent to the txpool prewarming worker.
pub(super) enum Command<J> {
    /// Starts prewarming for a canonical head, replacing any previous job.
    Start { parent_hash: B256, job: J },
    /// Pauses prewarming until a matching [`Resume`](Self::Resume).
    Pause,
    /// Releases one active pause.
    Resume,
}

/// One outstanding pause, released when dropped.
pub(super) struct PauseGuard<J> {
    control: Weak<Control<J>>,
}

impl<J> Drop for PauseGuard<J> {
    fn drop(&mut self) {
        // If the pause was never delivered the channel is disconnected, and this send fails
        // just the same, so the worker can never see an unmatched resume.
        if let Some(control) = self.control.upgrade() {
            let _ = control.commands.send(Command::Resume);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestControl = Control<u64>;

    fn control() -> (Arc<TestControl>, Receiver<Command<u64>>) {
        TestControl::new()
    }

    fn snapshot(parent_hash: B256) -> Snapshot {
        Snapshot::new(parent_hash, Default::default())
    }

    #[test]
    fn snapshot_is_published_for_matching_parent() {
        let (control, _receiver) = control();
        let parent_hash = B256::repeat_byte(0x01);
        *control.publication.write() = Some(snapshot(parent_hash));

        assert_eq!(
            control.snapshot(parent_hash).map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
        assert!(control.snapshot(B256::ZERO).is_none());
    }

    #[test]
    fn start_sends_job() {
        let (control, receiver) = control();
        let parent_hash = B256::repeat_byte(0x01);
        control.start(parent_hash, 1);

        assert!(matches!(
            receiver.try_recv(),
            Ok(Command::Start { parent_hash: received_parent, job: 1 })
                if received_parent == parent_hash
        ));
    }

    #[test]
    fn pause_queues_without_waiting_and_resumes_on_drop() {
        let (control, receiver) = control();
        // Nothing reads the channel, so this returning at all proves pause does not block.
        let guard = control.pause();
        assert!(matches!(receiver.try_recv(), Ok(Command::Pause)));

        drop(guard);
        assert!(matches!(receiver.try_recv(), Ok(Command::Resume)));
    }

    #[test]
    fn pause_guard_does_not_retain_control() {
        let (control, _receiver) = control();
        let weak_control = Arc::downgrade(&control);
        let guard = control.pause();

        drop(control);
        assert!(weak_control.upgrade().is_none());
        drop(guard);
    }

    #[test]
    fn overlapping_pause_guards_send_matching_resumes() {
        let (control, receiver) = control();
        let first = control.pause();
        let second = control.pause();
        assert!(matches!(receiver.try_recv(), Ok(Command::Pause)));
        assert!(matches!(receiver.try_recv(), Ok(Command::Pause)));

        drop(first);
        assert!(matches!(receiver.try_recv(), Ok(Command::Resume)));
        drop(second);
        assert!(matches!(receiver.try_recv(), Ok(Command::Resume)));
    }

    #[test]
    fn dropping_control_disconnects_worker() {
        let (control, receiver) = control();
        drop(control);

        assert!(receiver.recv().is_err());
    }
}
