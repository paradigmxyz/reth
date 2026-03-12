//! Custom allocator implementation.
//!
//! We provide support for mimalloc, jemalloc and snmalloc on unix systems. If multiple allocator
//! features are enabled, mimalloc is preferred, then jemalloc, then snmalloc.

// We provide multiple allocator options. If multiple features are enabled, mimalloc is prioritized,
// then jemalloc, then snmalloc.
cfg_if::cfg_if! {
    if #[cfg(feature = "mimalloc")] {
        type AllocatorInner = mimalloc::MiMalloc;
    } else if #[cfg(all(feature = "jemalloc", unix))] {
        type AllocatorInner = tikv_jemallocator::Jemalloc;
    } else if #[cfg(all(feature = "snmalloc", unix))] {
        type AllocatorInner = snmalloc_rs::SnMalloc;
    } else {
        type AllocatorInner = std::alloc::System;
    }
}

// This is to prevent clippy unused warnings when we do `--all-features`
cfg_if::cfg_if! {
    if #[cfg(all(feature = "snmalloc", any(feature = "jemalloc", feature = "mimalloc"), unix))] {
        use snmalloc_rs as _;
    }
}

cfg_if::cfg_if! {
    if #[cfg(all(feature = "jemalloc", feature = "mimalloc", unix))] {
        use tikv_jemallocator as _;
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
