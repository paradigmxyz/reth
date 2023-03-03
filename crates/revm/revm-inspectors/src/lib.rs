#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! revm [Inspector](revm::Inspector) implementations

/// An inspector implementation for an EIP2930 Accesslist
pub mod access_list;

/// An inspector stack abstracting the implementation details of
/// each inspector and allowing to hook on block/transaciton execution,
/// used in the main RETH executor.
pub mod stack;

/// An inspector for recording traces
pub mod tracing;
