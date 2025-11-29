use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use tracing_appender::non_blocking::{NonBlocking, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::fmt::MakeWriter;

/// Configuration for non-blocking writer behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NonBlockingBehavior {
    /// Drop messages when the buffer is full (default)
    #[default]
    Drop,
    /// Block the current thread when the buffer is full
    Block,
}

/// A custom non-blocking writer that tracks dropped messages and allows configurable behavior.
#[derive(Debug)]
pub struct NonBlockingDropTracking {
    inner: Mutex<NonBlocking>,
    drop_count: Arc<AtomicUsize>,
    behavior: NonBlockingBehavior,
}

impl NonBlockingDropTracking {
    /// Creates a new instance with the given inner writer and drop counter.
    pub const fn new(
        inner: NonBlocking,
        drop_count: Arc<AtomicUsize>,
        behavior: NonBlockingBehavior,
    ) -> Self {
        Self { inner: Mutex::new(inner), drop_count, behavior }
    }

    /// Returns the number of dropped messages since last reset.
    pub fn get_drop_count(&self) -> usize {
        self.drop_count.load(Ordering::Relaxed)
    }

    /// Resets the drop counter to zero.
    pub fn reset_drop_count(&self) {
        self.drop_count.store(0, Ordering::Relaxed);
    }

    /// Returns the current behavior configuration.
    pub const fn behavior(&self) -> NonBlockingBehavior {
        self.behavior
    }
}

impl io::Write for NonBlockingDropTracking {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.behavior {
            NonBlockingBehavior::Drop => {
                self.inner.lock().unwrap().write(buf).inspect_err(|e| {
                    if e.kind() == io::ErrorKind::WouldBlock {
                        self.drop_count.fetch_add(1, Ordering::Relaxed);
                        // Log the first drop to warn users immediately
                        if self.get_drop_count() == 1 {
                            eprintln!("WARNING: Log messages are being dropped due to full buffer. Consider increasing buffer size or using blocking behavior.");
                        }
                    }
                })
            }
            NonBlockingBehavior::Block => {
                // Implement blocking by retrying until written
                loop {
                    match self.inner.lock().unwrap().write(buf) {
                        Ok(n) => return Ok(n),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            // Yield to allow other threads to run
                            std::thread::yield_now();
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.lock().unwrap().flush()
    }
}

impl<'a> MakeWriter<'a> for NonBlockingDropTracking {
    type Writer = NonBlockingDropTrackingMakeWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        NonBlockingDropTrackingMakeWriter { inner: self }
    }
}

#[derive(Debug)]
pub struct NonBlockingDropTrackingMakeWriter<'a> {
    inner: &'a NonBlockingDropTracking,
}

impl<'a> io::Write for NonBlockingDropTrackingMakeWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Use interior mutability pattern instead of unsafe casting
        let mut inner_guard = self.inner.inner.lock().unwrap();
        inner_guard.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner_guard = self.inner.inner.lock().unwrap();
        inner_guard.flush()
    }
}

/// Builder for creating a non-blocking writer with drop tracking.
pub(crate) struct NonBlockingDropTrackingBuilder {
    inner_builder: NonBlockingBuilder,
    drop_count: Arc<AtomicUsize>,
    behavior: NonBlockingBehavior,
    buffer_size: usize,
}

impl Default for NonBlockingDropTrackingBuilder {
    fn default() -> Self {
        Self {
            inner_builder: NonBlockingBuilder::default(),
            drop_count: Arc::new(AtomicUsize::new(0)),
            behavior: NonBlockingBehavior::default(),
            buffer_size: 1000, // Default buffer size
        }
    }
}

impl NonBlockingDropTrackingBuilder {
    /// Creates a new builder with default settings.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Sets the buffer size in number of lines.
    pub(crate) fn buffered_lines_limit(mut self, limit: usize) -> Self {
        self.inner_builder = self.inner_builder.buffered_lines_limit(limit);
        self.buffer_size = limit;
        self
    }

    /// Sets the behavior for when the buffer is full.
    pub(crate) const fn behavior(mut self, behavior: NonBlockingBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    /// Builds the non-blocking writer with drop tracking.
    pub(crate) fn build(
        self,
        writer: impl io::Write + Send + 'static,
    ) -> (NonBlockingDropTracking, WorkerGuard) {
        let (non_blocking, guard) = NonBlocking::new(writer);
        let non_blocking_drop_tracking =
            NonBlockingDropTracking::new(non_blocking, self.drop_count, self.behavior);
        (non_blocking_drop_tracking, guard)
    }
}
