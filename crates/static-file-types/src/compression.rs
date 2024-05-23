use strum::AsRefStr;

/// Static File compression types.
#[derive(Debug, Copy, Clone, Default, AsRefStr)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Compression {
    /// LZ4 compression algorithm.
    #[strum(serialize = "lz4")]
    Lz4,
    /// Zstandard (Zstd) compression algorithm.
    #[strum(serialize = "zstd")]
    Zstd,
    /// Zstandard (Zstd) compression algorithm with a dictionary.
    #[strum(serialize = "zstd-dict")]
    ZstdWithDictionary,
    /// No compression.
    #[strum(serialize = "uncompressed")]
    #[default]
    Uncompressed,
}
