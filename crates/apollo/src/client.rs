//! Apollo configuration client for dynamic configuration management.

use crate::types::{ApolloConfig, ApolloError, ConfigValue};
use apollo_sdk::client::apollo_config_client::ApolloConfigClient;
use async_once_cell::OnceCell;
use moka::sync::Cache;
use serde_json::Value as JsonValue;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Apollo client wrapper for reth
pub struct ApolloService {
    inner: Arc<ApolloConfigClient>,
    config: ApolloConfig,
    namespace_map: HashMap<String, String>,
    cache: Cache<String, ConfigValue>,
    listener_state: Arc<Mutex<ListenerState>>,
}

#[derive(Debug)]
struct ListenerState {
    task: Option<tokio::task::JoinHandle<()>>,
}

/// Singleton instance
static INSTANCE: OnceCell<Arc<ApolloService>> = OnceCell::new();

// Constants
const POLLING_INTERVAL: Duration = Duration::from_millis(1000);
const CACHE_MAX_CAPACITY: u64 = 1000;

impl std::fmt::Debug for ApolloService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApolloService")
            .field("config", &self.config)
            .field("namespace_map", &self.namespace_map)
            .field("cache", &self.cache)
            .field("listener_state", &"<locked>")
            .finish()
    }
}

impl Clone for ApolloService {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            config: self.config.clone(),
            namespace_map: self.namespace_map.clone(),
            cache: self.cache.clone(),
            listener_state: self.listener_state.clone(),
        }
    }
}

#[allow(dead_code)]
impl ApolloService {
    /// Get singleton instance
    pub async fn try_initialize(config: ApolloConfig) -> Result<Arc<ApolloService>, ApolloError> {
        let instance = INSTANCE
            .get_or_try_init(async {
                let client = Self::try_new_instance(config).await?;
                Ok(Arc::new(client))
            })
            .await?;

        // Start listening on the singleton instance
        instance.start_listening().await?;

        Ok(instance.clone())
    }

    /// Create new instance
    async fn try_new_instance(config: ApolloConfig) -> Result<ApolloService, ApolloError> {
        // Validate configuration
        if config.app_id.is_empty() ||
            config.meta_server.is_empty() ||
            config.cluster_name.is_empty()
        {
            return Err(ApolloError::ClientInit(
                "apollo enabled but config is not valid".to_string(),
            ));
        }

        info!(target: "reth::apollo", "[Apollo] Namespaces: {:?}", config.namespaces);

        let client: ApolloConfigClient = apollo_sdk::client::apollo_config_client::new(
            config.meta_server.iter().map(|s| s.as_str()).collect(),
            &config.app_id,
            &config.cluster_name,
            config.namespaces.as_deref().map(|ns| ns.iter().map(|s| s.as_str()).collect()),
            config.secret.as_deref(),
        )
        .await
        .map_err(|e| {
            error!(target: "reth::apollo", "[Apollo] Failed to create client: {:?}", e);
            ApolloError::ClientInit(format!("Failed to connect to Apollo: Check if Apollo service is accessible or whether Apollo configuration for desired namespace is released."))
        })?;

        // Create namespace map
        let mut namespace_map = HashMap::new();
        if let Some(namespaces) = &config.namespaces {
            for full_namespace in namespaces {
                let namespace = get_namespace(full_namespace)?;
                if namespace_map.contains_key(&namespace) {
                    return Err(ApolloError::ClientInit(format!(
                        "duplicate apollo namespace: {}",
                        namespace
                    )));
                }
                namespace_map.insert(namespace, full_namespace.clone());
            }
        }

        Ok(ApolloService {
            inner: Arc::new(client),
            config,
            namespace_map,
            cache: Cache::builder().max_capacity(CACHE_MAX_CAPACITY).build(),
            listener_state: Arc::new(Mutex::new(ListenerState { task: None })),
        })
    }

    /// Get singleton instance
    pub fn get_instance() -> Result<Arc<ApolloService>, ApolloError> {
        INSTANCE
            .get()
            .cloned()
            .ok_or(ApolloError::ClientInit("Apollo client not initialized".to_string()))
    }

    async fn load_config(&self) -> Result<(), ApolloError> {
        for (namespace, full_namespace) in &self.namespace_map {
            let config = self
                .inner
                .get_config_from_namespace("content", full_namespace)
                .map(|c| c.config_value.clone());
            if let Some(config) = config {
                // Get config cache for namespace
                Self::update_cache_from_config(&self.cache, namespace, &config);
            } else {
                warn!(target: "reth::apollo", "[Apollo] No config found for namespace {}", namespace);
            }
        }

        Ok(())
    }

    async fn start_listening(&self) -> Result<(), ApolloError> {
        let mut state = self.listener_state.lock().await;
        if state.task.is_some() {
            return Ok(());
        }

        let client = self.inner.clone();
        let cache = self.cache.clone();
        let namespace_map = self.namespace_map.clone();

        // Start listening to all namespaces
        for (namespace, full_namespace) in &namespace_map {
            if let Some(err) = client.listen_namespace(full_namespace).await {
                warn!(target: "reth::apollo", "[Apollo] Failed to listen to namespace {}: {:?}", namespace, err);
            }
        }

        // Load initial config
        self.load_config().await?;

        // Spawn background listener task
        let task = tokio::spawn(async move {
            Self::listener_task(client, cache, namespace_map).await;
        });

        state.task = Some(task);

        info!(target: "reth::apollo", "[Apollo] Started listening to configuration changes");
        Ok(())
    }

    async fn listener_task(
        client: Arc<ApolloConfigClient>,
        cache: Cache<String, ConfigValue>,
        namespace_map: HashMap<String, String>,
    ) {
        let mut interval = tokio::time::interval(POLLING_INTERVAL);
        loop {
            interval.tick().await;

            if let Some(change_event) = client.fetch_change_event() {
                info!(target: "reth::apollo", "[Apollo] Configuration change detected: {:?}", change_event);
                Self::fetch_and_update_configs(&client, &cache, &namespace_map).await;
            }
        }
    }

    async fn fetch_and_update_configs(
        client: &Arc<ApolloConfigClient>,
        cache: &Cache<String, ConfigValue>,
        namespace_map: &HashMap<String, String>,
    ) {
        for (namespace, full_namespace) in namespace_map {
            let config = client
                .get_config_from_namespace("content", full_namespace)
                .map(|c| c.config_value.clone());

            if let Some(config) = config {
                Self::update_cache_from_config(&cache, namespace, &config);
            } else {
                warn!(target: "reth::apollo", "[Apollo] get_config returned None for namespace {}. This may happen if the namespace format doesn't match.", namespace);
            }
        }
    }

    fn update_cache_from_config(
        cache: &Cache<String, ConfigValue>,
        namespace: &str,
        config_value: &str,
    ) {
        match serde_yaml::from_str::<HashMap<String, JsonValue>>(config_value) {
            Ok(parsed_config) => {
                for (key, value) in parsed_config {
                    cache.insert(
                        make_cache_key(namespace, key.as_str()),
                        ConfigValue::try_from_json(&value)
                            .unwrap_or_else(|| ConfigValue::String(value.to_string())),
                    );
                }
            }
            Err(e) => {
                error!(target: "reth::apollo", "[Apollo] Failed to parse YAML for namespace {}: {}", namespace, e);
            }
        }
    }

    /// Try to get cached config from cache
    pub fn try_get_cached_config(&self, namespace: &str, key: &str) -> Option<ConfigValue> {
        let cache_key = make_cache_key(namespace, key);
        debug!(target: "reth::apollo", "[Apollo] Getting cached config for namespace {}: key: {:?}", namespace, cache_key);
        self.cache.get(&cache_key)
    }
}

/// Get namespace from full namespace name
fn get_namespace(namespace: &str) -> Result<String, ApolloError> {
    namespace
        .split('-')
        .next()
        .ok_or_else(|| ApolloError::InvalidNamespace(namespace.to_string()))
        .map(|s| s.to_string())
}

fn make_cache_key(namespace: &str, key: &str) -> String {
    format!("{}:{}", namespace, key)
}
