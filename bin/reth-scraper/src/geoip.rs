use maxminddb::Reader;
use std::{net::IpAddr, sync::Arc};

#[derive(Clone)]
pub struct GeoIpResolver {
    reader: Option<Arc<Reader<Vec<u8>>>>,
}

impl GeoIpResolver {
    pub fn new(db_path: Option<&str>) -> eyre::Result<Self> {
        let paths = if let Some(path) = db_path {
            vec![path.to_string()]
        } else {
            vec![
                "./GeoLite2-Country.mmdb".to_string(),
                "/usr/share/GeoIP/GeoLite2-Country.mmdb".to_string(),
                format!(
                    "{}/.local/share/GeoIP/GeoLite2-Country.mmdb",
                    std::env::var("HOME").unwrap_or_default()
                ),
            ]
        };

        for path in paths {
            if let Ok(reader) = Reader::open_readfile(&path) {
                tracing::info!("Loaded GeoIP database from {}", path);
                return Ok(Self { reader: Some(Arc::new(reader)) });
            }
        }

        tracing::warn!("No GeoIP database found, country lookups will be disabled");
        Ok(Self { reader: None })
    }

    pub fn lookup(&self, ip: IpAddr) -> Option<String> {
        let reader = self.reader.as_ref()?;
        let result: maxminddb::geoip2::Country<'_> = reader.lookup(ip).ok()?;
        result.country?.iso_code.map(|s| s.to_string())
    }
}
