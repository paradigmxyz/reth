//! Custom allocator implementation.

// We use jemalloc for performance reasons.
cfg_if::cfg_if! {
    if #[cfg(all(feature = "jemalloc", unix))] {
        type AllocatorInner = tikv_jemallocator::Jemalloc;
    } else {
        type AllocatorInner = std::alloc::System;
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
