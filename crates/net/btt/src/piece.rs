//! Represents a single piece of the torrent

use bytes::Bytes;

/// smallest allowed piece size : 16 KB
pub const BLOCK_SIZE_MIN: usize = 16384;

/// greatest allowed piece size: 16 MB
pub const BLOCK_SIZE_MAX: usize = 16777216;

/// Represents (part of) the content of a piece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// Specifying the zero-based piece index
    pub index: u32,
    /// Specifying the zero-based byte offset within the piece
    pub begin: u32,
    /// Block of data, which is a subset of the piece specified by index.
    pub data: Bytes,
}

/// Represents whether the peer owns the piece or not.
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PieceOwnerShip {
    /// Peer does _not_ have the piece.
    Missing = 0,
    /// Peer does have the piece.
    Owned = 1,
}

/// How pieces should be selected
#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PieceSelection {
    /// Select a random piece
    Random,
    /// Once peers finish downloading the current piece, it will select the next
    /// piece which is the fewest among its neighbors
    Rarest,
}

impl Default for PieceSelection {
    fn default() -> Self {
        PieceSelection::Random
    }
}

// #[derive(Debug, Clone, Eq, PartialEq)]
// pub struct IndexRange {
//     pub begin: usize,
//     pub end: usize,
// }

/// Represents the status of a piece, after checking its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PieceState {
    /// Piece was discovered as good.
    Good(u64),
    /// Piece was discovered as bad.
    Bad(u64),
}
