//! Consist of types adjacent to the fee history cache and its configs

use alloy_rpc_client::RpcClient;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Debug;

/// Configuration for the transaction forwarder.
#[derive(Debug, Clone, Default)]
pub struct ForwardConfig {
    /// The raw transaction forwarder.
    ///
    /// Default is `None`
    pub tx_forwarder: Option<RpcClient>,
}

impl Eq for ForwardConfig {}

impl PartialEq for ForwardConfig {
    fn eq(&self, other: &Self) -> bool {
        matches!((&self.tx_forwarder, &other.tx_forwarder), (None, None))
    }
}

impl Serialize for ForwardConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ForwardConfig", 1)?;

        match &self.tx_forwarder {
            Some(_) => state.serialize_field("tx_forwarder", &"<rpc_client>")?,
            None => state.serialize_field("tx_forwarder", &Option::<String>::None)?,
        }

        state.end()
    }
}

impl<'de> Deserialize<'de> for ForwardConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            tx_forwarder: Option<String>,
        }

        let helper = Helper::deserialize(deserializer)?;

        let tx_forwarder = helper
            .tx_forwarder
            .map(|url| {
                reqwest::Url::parse(&url)
                    .map_err(|e| serde::de::Error::custom(format!("Invalid URL: {e}")))
                    .map(RpcClient::new_http)
            })
            .transpose()?;

        Ok(Self { tx_forwarder })
    }
}
