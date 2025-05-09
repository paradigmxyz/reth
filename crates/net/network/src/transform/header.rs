//! Abstraction over a transformation applied to headers.

use reth_primitives_traits::BlockHeader;

/// An instance of the trait applies a mapping to the input header.
pub trait HeaderTransform<H: BlockHeader>: std::fmt::Debug + Send + Sync {
    /// Applies a mapping to the input header.
    fn map(&self, header: H) -> H;
}

impl<H: BlockHeader> HeaderTransform<H> for () {
    fn map(&self, header: H) -> H {
        header
    }
}
