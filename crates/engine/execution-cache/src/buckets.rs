//! Memory of the execution cache tables.
//!
//! The tables start zeroed and are read before they are written. Reading a page of a fresh
//! anonymous mapping maps it to the kernel's shared zero page, and the first write to it then
//! takes a copy-on-write fault that interrupts every core running the process to flush the old
//! translation from its TLB. Across a multi-gigabyte table that happens for every page a block
//! touches first, on the threads that execute the block. On Linux the tables are therefore mapped
//! here, backed by huge pages where available, and populated writable on a background thread.

use fixed_cache::Bucket;

/// Bucket array of one cache table.
pub(crate) struct BucketMemory<T> {
    inner: imp::Mapping<T>,
}

impl<T: 'static> BucketMemory<T> {
    /// Maps `len` empty buckets, or returns `None` if the platform or the kernel cannot.
    pub(crate) fn new(len: usize) -> Option<Self> {
        imp::Mapping::new(len).map(|inner| Self { inner })
    }

    /// Returns the buckets with an unbounded lifetime.
    ///
    /// # Safety
    ///
    /// The returned slice must not be used after `self` is dropped.
    pub(crate) const unsafe fn buckets(&self) -> &'static [Bucket<T>] {
        // SAFETY: the caller does not use the slice after `self` is dropped.
        unsafe { self.inner.buckets() }
    }
}

impl<T> core::fmt::Debug for BucketMemory<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BucketMemory").finish_non_exhaustive()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Bucket;
    use std::{ptr::NonNull, thread::JoinHandle};

    /// Huge page size the mapping is rounded up to.
    const HUGE_PAGE: usize = 2 << 20;

    /// Bytes populated or unmapped per call. Both hold the process' memory map lock, which blocks
    /// every other thread that maps or unmaps memory, including the allocator, so the mapping is
    /// handled in chunks.
    const CHUNK: usize = 64 << 20;

    /// An anonymous mapping holding `len` buckets.
    pub(super) struct Mapping<T> {
        buckets: NonNull<Bucket<T>>,
        len: usize,
        bytes: usize,
        /// Populates the mapping writable; joined before the mapping is unmapped.
        populate: Option<JoinHandle<()>>,
    }

    // SAFETY: the mapping is only accessed through `Bucket`s, which synchronize their contents.
    unsafe impl<T: Send> Send for Mapping<T> {}
    // SAFETY: as above.
    unsafe impl<T: Send> Sync for Mapping<T> {}

    impl<T: 'static> Mapping<T> {
        pub(super) fn new(len: usize) -> Option<Self> {
            let bytes =
                core::mem::size_of::<Bucket<T>>().checked_mul(len)?.next_multiple_of(HUGE_PAGE);
            // SAFETY: an anonymous private mapping with no address hint has no preconditions.
            let ptr = unsafe {
                libc::mmap(
                    core::ptr::null_mut(),
                    bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                    -1,
                    0,
                )
            };
            if ptr == libc::MAP_FAILED {
                return None
            }
            // Huge pages cut the number of faults and TLB misses; the kernel may decline.
            // SAFETY: `ptr..ptr + bytes` is the mapping created above.
            unsafe { libc::madvise(ptr, bytes, libc::MADV_HUGEPAGE) };
            let address = ptr as usize;
            // Populating does not change the contents, so it may run while the table is in use.
            let populate = std::thread::Builder::new()
                .name("cache-populate".to_string())
                .spawn(move || {
                    for offset in (0..bytes).step_by(CHUNK) {
                        // SAFETY: the chunk lies in the mapping, which outlives this thread: the
                        // thread is joined before the mapping is unmapped.
                        unsafe {
                            libc::madvise(
                                (address + offset) as *mut libc::c_void,
                                CHUNK.min(bytes - offset),
                                libc::MADV_POPULATE_WRITE,
                            )
                        };
                    }
                })
                .ok();
            // Zeroed memory is a valid array of empty buckets, as `Bucket::new` is all zeros.
            Some(Self { buckets: NonNull::new(ptr.cast())?, len, bytes, populate })
        }

        pub(super) const unsafe fn buckets(&self) -> &'static [Bucket<T>] {
            // SAFETY: the mapping holds `len` initialized buckets and the caller does not use the
            // slice after `self` is dropped.
            unsafe { core::slice::from_raw_parts(self.buckets.as_ptr(), self.len) }
        }
    }

    impl<T> Drop for Mapping<T> {
        fn drop(&mut self) {
            if let Some(populate) = self.populate.take() {
                let _ = populate.join();
            }
            // SAFETY: the buckets are initialized, no longer borrowed, and dropped once, then the
            // mapping created in `new` is unmapped.
            unsafe {
                core::ptr::drop_in_place(core::ptr::slice_from_raw_parts_mut(
                    self.buckets.as_ptr(),
                    self.len,
                ));
                let address = self.buckets.as_ptr() as usize;
                for offset in (0..self.bytes).step_by(CHUNK) {
                    libc::munmap(
                        (address + offset) as *mut libc::c_void,
                        CHUNK.min(self.bytes - offset),
                    );
                }
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Bucket;

    /// Mapping tables is only supported on Linux.
    pub(super) struct Mapping<T>(core::marker::PhantomData<T>);

    impl<T: 'static> Mapping<T> {
        pub(super) const fn new(_len: usize) -> Option<Self> {
            None
        }

        pub(super) const unsafe fn buckets(&self) -> &'static [Bucket<T>] {
            &[]
        }
    }
}
