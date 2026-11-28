//! eth/72 blob transactions, whose bodies arrive without blobs and whose cells are fetched
//! separately.

mod buffer;
mod fetcher;

pub(crate) use fetcher::BlobFetcher;
