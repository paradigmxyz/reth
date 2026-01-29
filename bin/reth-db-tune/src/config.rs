use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub(crate) struct TuningConfig {
    #[serde(default)]
    pub default: CfOptions,
    #[serde(default)]
    pub cf: HashMap<String, CfOptions>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub(crate) struct CfOptions {
    pub write_buffer_size: Option<String>,
    pub max_write_buffer_number: Option<i32>,
    pub compression: Option<CompressionType>,
    pub block_size: Option<String>,
    pub bloom_filter_bits: Option<i32>,
    pub max_background_jobs: Option<i32>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompressionType {
    None,
    Snappy,
    Lz4,
    Zstd,
}

impl From<CompressionType> for rocksdb::DBCompressionType {
    fn from(ct: CompressionType) -> Self {
        match ct {
            CompressionType::None => Self::None,
            CompressionType::Snappy => Self::Snappy,
            CompressionType::Lz4 => Self::Lz4,
            CompressionType::Zstd => Self::Zstd,
        }
    }
}

pub(crate) fn parse_size(s: &str) -> eyre::Result<usize> {
    let s = s.trim();
    let (num_str, multiplier) = if let Some(n) = s.strip_suffix("GiB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KiB") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix("GB") {
        (n, 1_000_000_000)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n, 1_000_000)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n, 1000)
    } else {
        (s, 1)
    };

    let num: usize = num_str.trim().parse()?;
    Ok(num * multiplier)
}

impl TuningConfig {
    pub(crate) fn get_cf_options(&self, cf_name: &str) -> CfOptions {
        let mut opts = self.default.clone();

        if let Some(cf_opts) = self.cf.get(cf_name) {
            if cf_opts.write_buffer_size.is_some() {
                opts.write_buffer_size = cf_opts.write_buffer_size.clone();
            }
            if cf_opts.max_write_buffer_number.is_some() {
                opts.max_write_buffer_number = cf_opts.max_write_buffer_number;
            }
            if cf_opts.compression.is_some() {
                opts.compression = cf_opts.compression;
            }
            if cf_opts.block_size.is_some() {
                opts.block_size = cf_opts.block_size.clone();
            }
            if cf_opts.bloom_filter_bits.is_some() {
                opts.bloom_filter_bits = cf_opts.bloom_filter_bits;
            }
            if cf_opts.max_background_jobs.is_some() {
                opts.max_background_jobs = cf_opts.max_background_jobs;
            }
        }

        opts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
[default]
write_buffer_size = "256MiB"
max_write_buffer_number = 4
compression = "lz4"
block_size = "16KiB"
bloom_filter_bits = 10
max_background_jobs = 16

[cf.Headers]
compression = "zstd"
bloom_filter_bits = 12

[cf.TransactionLookup]
write_buffer_size = "512MiB"
"#;

        let config: TuningConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default.max_write_buffer_number, Some(4));
        assert_eq!(config.default.compression, Some(CompressionType::Lz4));

        let headers_opts = config.get_cf_options("Headers");
        assert_eq!(headers_opts.compression, Some(CompressionType::Zstd));
        assert_eq!(headers_opts.bloom_filter_bits, Some(12));
        assert_eq!(headers_opts.max_write_buffer_number, Some(4));

        let tx_opts = config.get_cf_options("TransactionLookup");
        assert_eq!(tx_opts.write_buffer_size, Some("512MiB".to_string()));
        assert_eq!(tx_opts.compression, Some(CompressionType::Lz4));
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("256MiB").unwrap(), 256 * 1024 * 1024);
        assert_eq!(parse_size("16KiB").unwrap(), 16 * 1024);
        assert_eq!(parse_size("1GiB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_size("1000").unwrap(), 1000);
    }
}
