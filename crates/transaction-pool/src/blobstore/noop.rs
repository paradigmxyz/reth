/// A blobstore implementation that does nothing
#[derive(Clone, Copy, Debug, PartialOrd, PartialEq, Default)]
#[non_exhaustive]
pub struct NoopBlobStore;
