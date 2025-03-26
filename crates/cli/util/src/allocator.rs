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

// This is to prevent clippy unused warnings when we do `--all-features`
cfg_if::cfg_if! {
    if #[cfg(all(feature = "snmalloc", feature = "jemalloc", unix))] {
        use snmalloc_rs as _;
    }
}

cfg_if::cfg_if! {
    if #[cfg(feature = "tracy-allocator")] {
        type AllocatorWrapper = tracy_client::ProfiledAllocator<AllocatorInner>;
        tracy_client::register_demangler!();
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
