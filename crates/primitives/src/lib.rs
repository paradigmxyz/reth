//! Commonly used types in Reth.
//!
//! ## Deprecation Notice
//!
//! This crate is deprecated and will be removed in a future release.
//! Use [`reth-ethereum-primitives`](https://crates.io/crates/reth-ethereum-primitives) and
//! [`reth-primitives-traits`](https://crates.io/crates/reth-primitives-traits) instead.

#![cfg_attr(
    not(feature = "__internal"),
    deprecated(
        note = "the `reth-primitives` crate is deprecated, use `reth-ethereum-primitives` and `reth-primitives-traits` instead."
    )
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(feature = "std"), no_std)]
