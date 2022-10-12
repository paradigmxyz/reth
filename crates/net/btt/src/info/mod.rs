//! This module contains a type safe representation of a torrent's metainfo, as
//! well as utilities to construct it.
//!
//! This module is adapted from <https://github.com/mandreyel/cratetorrent/commit/34aa13835872a14f00d4a334483afff79181999f>

pub mod meta;
pub mod storage;

pub use meta::Metainfo;
pub use storage::{FileInfo, StorageInfo};

/// Index of a file in the torrent.
pub(crate) type FileIndex = usize;

/// Index of a piece in the torrent
pub(crate) type PieceIndex = usize;
