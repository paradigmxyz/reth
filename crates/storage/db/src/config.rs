/// Configuration the database
///
/// This allows for the configuration of the database via [Environment](reth_libmdbx::Environment)
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// The maximum number of concurrent readers for the database.
    ///
    /// See also [set_max_readers](reth_libmdbx::EnvironmentBuilder::set_max_readers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_readers: Option<usize>,
}
