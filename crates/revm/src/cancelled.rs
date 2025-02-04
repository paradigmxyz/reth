use alloc::sync::Arc;
use core::sync::atomic::AtomicBool;

/// A marker that can be used to cancel execution.
///
/// If dropped, it will set the `cancelled` flag to true.
///
/// This is most useful when a payload job needs to be cancelled.
#[derive(Default, Clone, Debug)]
pub struct CancelOnDrop(Arc<AtomicBool>);

// === impl CancelOnDrop ===

impl CancelOnDrop {
    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, core::sync::atomic::Ordering::Relaxed);
    }
}

/// A marker that can be used to cancel execution.
///
/// If dropped, it will NOT set the `cancelled` flag to true.
/// If `cancel` is called, the `cancelled` flag will be set to true.
///
/// This is useful in prewarming, when an external signal is received to cancel many prewarming
/// tasks.
#[derive(Default, Clone, Debug)]
pub struct ManualCancel(Arc<AtomicBool>);

// === impl ManualCancel ===

impl ManualCancel {
    /// Returns true if the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Drops the [`ManualCancel`], setting the cancelled flag to true.
    pub fn cancel(self) {
        self.0.store(true, core::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_cancelled() {
        let c = CancelOnDrop::default();
        assert!(!c.is_cancelled());
    }

    #[test]
    fn test_default_cancel_task() {
        let c = ManualCancel::default();
        assert!(!c.is_cancelled());
    }

    #[test]
    fn test_set_cancel_task() {
        let c = ManualCancel::default();
        assert!(!c.is_cancelled());
        let c2 = c.clone();
        let c3 = c.clone();
        c.cancel();
        assert!(c3.is_cancelled());
        assert!(c2.is_cancelled());
    }

    #[test]
    fn test_cancel_task_multiple_threads() {
        let c = ManualCancel::default();
        let cloned_cancel = c.clone();

        // we want to make sure that:
        // * we can spawn tasks that do things
        // * those tasks can run to completion and the flag remains unset unless we call cancel
        let mut handles = vec![];
        for _ in 0..10 {
            let c = c.clone();
            let handle = std::thread::spawn(move || {
                for _ in 0..1000 {
                    if c.is_cancelled() {
                        return;
                    }
                }
            });
            handles.push(handle);
        }

        // wait for all the threads to finish
        for handle in handles {
            handle.join().unwrap();
        }

        // check that the flag is still unset
        assert!(!c.is_cancelled());

        // cancel and check that the flag is set
        c.cancel();
        assert!(cloned_cancel.is_cancelled());
    }
}
