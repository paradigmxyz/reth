#[derive(Debug, Copy, Clone, Default)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Compression {
    Lz4,
    Zstd,
    ZstdWithDictionary,
    #[default]
    Uncompressed,
}

impl Compression {
    pub const fn file_name(&self) -> &'static str {
        match self {
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
            Self::ZstdWithDictionary => "zstd-dict",
            Self::Uncompressed => "uncompressed",
        }
    }
}
