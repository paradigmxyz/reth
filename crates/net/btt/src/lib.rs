#![warn(missing_debug_implementations, missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![allow(unused)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Reth bittorrent support.
//!
//! # Design:
//!
//! - TorrentPool: tracks the state of all torrents
//! - Torrent: drives a torrent to completion
//! - DiskManager: file IO
//! - Peer: session for connected peer

pub mod bitfield;
pub mod bittorrent;
pub mod block;
pub mod disk;
pub mod error;
pub mod info;
pub mod peer;
pub mod piece;
pub mod proto;
pub mod sha1;
pub mod torrent;
pub mod tracker;
