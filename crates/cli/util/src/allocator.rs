//! Custom allocator implementation.
//!
//! We provide support for jemalloc and snmalloc on unix systems, and prefer jemalloc if both are
//! enabled.

// We provide jemalloc allocator support, alongside snmalloc. If both features are enabled, jemalloc
// is prioritized.
cfg_if::cfg_if! {
    if #[cfg(all(feature = "jemalloc", unix))] {
        type AllocatorInner = tikv_jemallocator::Jemalloc;
    } else if #[cfg(all(feature = "snmalloc", unix))] {
        type AllocatorInner = snmalloc_rs::SnMalloc;
    } else {
        type AllocatorInner = std::alloc::System;
    }
}

// Re-export jemalloc-sys so that binaries can `use` it in main.rs to make it
// visible to the linker, which is required for `override_allocator_on_supported_platforms`.
#[cfg(all(feature = "jemalloc", unix))]
pub use tikv_jemalloc_sys;

/// Disables jemalloc arena decay so freed pages stay resident for reuse.
///
/// Payload processing allocates a burst of working memory every block; with default decay,
/// arenas release those pages between blocks (`madvise`) and the next block pays a minor
/// fault plus kernel page-zeroing to get them back — a measurable per-block kernel-time cost
/// on the engine thread. Keeping dirty pages resident avoids that cycle; the retained
/// footprint is bounded by each arena's high-water mark.
///
/// Call early in `main`, before worker threads spawn. A `malloc_conf` link-time override is
/// not used because fat LTO can internalize the exported symbol, silently reverting to
/// defaults; `mallctl` is unaffected by link options.
#[cfg(all(feature = "jemalloc", unix))]
pub fn disable_jemalloc_decay() {
    /// `MALLCTL_ARENAS_ALL`: applies a per-arena control to all existing arenas.
    const ARENAS_ALL: usize = 4096;
    let never: isize = -1;
    // Default for arenas created later, then retune arenas that already exist.
    for name in [b"arenas.dirty_decay_ms\0".as_slice(), b"arenas.muzzy_decay_ms\0".as_slice()] {
        // SAFETY: mallctl name is a valid NUL-terminated string and the value type
        // (ssize_t) matches the control's documented type.
        let _ = unsafe { tikv_jemalloc_ctl::raw::write(name, never) };
    }
    for suffix in ["dirty_decay_ms", "muzzy_decay_ms"] {
        let name = format!("arena.{ARENAS_ALL}.{suffix}\0");
        // SAFETY: as above.
        let _ = unsafe { tikv_jemalloc_ctl::raw::write(name.as_bytes(), never) };
    }
}

// This is to prevent clippy unused warnings when we do `--all-features`
cfg_if::cfg_if! {
    if #[cfg(all(feature = "snmalloc", feature = "jemalloc", unix))] {
        use snmalloc_rs as _;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tracy-allocator")] {
        type AllocatorWrapper = tracy_client::ProfiledAllocator<AllocatorInner>;
        const fn new_allocator_wrapper() -> AllocatorWrapper {
            AllocatorWrapper::new(AllocatorInner {}, 100)
        }
    } else {
        type AllocatorWrapper = AllocatorInner;
        const fn new_allocator_wrapper() -> AllocatorWrapper {
            AllocatorInner {}
        }
    }
}

/// Custom allocator.
pub type Allocator = AllocatorWrapper;

/// Creates a new [custom allocator][Allocator].
pub const fn new_allocator() -> Allocator {
    new_allocator_wrapper()
}
