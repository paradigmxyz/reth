//! Perform DNS lookups

use async_trait::async_trait;
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

/// A type that can lookup DNS entries
#[async_trait]
pub trait Resolver: Send + Sync {
    /// Performs a textual lookup.
    async fn lookup_txt(&self, query: &str) -> Option<String>;
}

/// A [Resolver] that uses an in memory map to lookup entries
#[derive(Debug, Clone)]
pub struct MapResolver(HashMap<String, String>);

impl Deref for MapResolver {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MapResolver {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl Resolver for MapResolver {
    async fn lookup_txt(&self, query: &str) -> Option<String> {
        self.get(query).cloned()
    }
}
