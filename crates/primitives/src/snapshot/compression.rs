use strum::AsRefStr;

#[derive(Debug, Copy, Clone, Default, AsRefStr)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[allow(missing_docs)]
/// Snapshot compression
pub enum Compression {
    #[strum(serialize = "lz4")]
    Lz4,
    #[strum(serialize = "zstd")]
    Zstd,
    #[strum(serialize = "zstd-dict")]
    ZstdWithDictionary,
    #[strum(serialize = "uncompressed")]
    #[default]
    Uncompressed,
}
