#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![allow(unused)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth bittorrent support.

pub mod alert;
pub(crate) mod avg;
pub mod bitfield;
mod bittorrent;
pub mod block;
pub(crate) mod counter;
pub mod disk;
pub(crate) mod download;
pub mod error;
pub(crate) mod info;
pub mod peer;
pub mod piece;
pub mod proto;
pub mod sha1;
pub mod torrent;
pub mod tracker;

pub use bittorrent::*;

#[cfg(target_os = "linux")]
pub(crate) mod iovecs;
