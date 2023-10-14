#[derive(Debug, Copy, Clone, Default)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[allow(missing_docs)]
/// Snapshot compression
pub enum Compression {
    Lz4,
    Zstd,
    ZstdWithDictionary,
    #[default]
    Uncompressed,
}
