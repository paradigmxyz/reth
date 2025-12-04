use crate::error::EthStatsError;
use std::str::FromStr;

/// Credentials for connecting to an `EthStats` server
///
/// Contains the node identifier, authentication secret, and server host
/// information needed to establish a connection with the `EthStats` service.
#[derive(Debug, Clone)]
pub(crate) struct EthstatsCredentials {
    /// Unique identifier for this node in the `EthStats` network
    pub node_id: String,
    /// Authentication secret for the `EthStats` server
    pub secret: String,
    /// Host address of the `EthStats` server
    pub host: String,
}

impl FromStr for EthstatsCredentials {
    type Err = EthStatsError;

    /// Parse credentials from a string in the format "`node_id:secret@host`"
    ///
    /// # Arguments
    /// * `s` - String containing credentials in the format "`node_id:secret@host`"
    ///
    /// # Returns
    /// * `Ok(EthstatsCredentials)` - Successfully parsed credentials
    /// * `Err(EthStatsError::InvalidUrl)` - Invalid format or missing separators
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('@').collect();
        if parts.len() != 2 {
            return Err(EthStatsError::InvalidUrl("Missing '@' separator".to_string()));
        }
        let creds = parts[0];
        let host = parts[1].to_string();
        let creds_parts: Vec<&str> = creds.split(':').collect();
        if creds_parts.len() != 2 {
            return Err(EthStatsError::InvalidUrl(
                "Missing ':' separator in credentials".to_string(),
            ));
        }
        let node_id = creds_parts[0].to_string();
        let secret = creds_parts[1].to_string();

        Ok(Self { node_id, secret, host })
    }
}
