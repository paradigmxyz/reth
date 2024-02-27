use crate::tree::{LinkEntry, TreeRootEntry};
use enr::EnrKeyUnambiguous;
use linked_hash_set::LinkedHashSet;
use metrics::Gauge;
use reth_metrics::Metrics;
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// A sync-able tree
pub(crate) struct SyncTree<K: EnrKeyUnambiguous = SecretKey> {
    /// Root of the tree
    root: TreeRootEntry,
    /// Link to this tree
    link: LinkEntry<K>,
    /// Timestamp when the root was updated
    root_updated: Instant,
    /// The state of the tree sync progress.
    sync_state: SyncState,
    /// Links contained in this tree
    resolved_links: HashMap<String, LinkEntry<K>>,
    /// Unresolved links of the tree
    unresolved_links: LinkedHashSet<String>,
    /// Unresolved nodes of the tree
    unresolved_nodes: LinkedHashSet<String>,
    /// metrics
    metrics: SyncTreeMetrics,
}

// === impl SyncTree ===

impl<K: EnrKeyUnambiguous> SyncTree<K> {
    pub(crate) fn new(root: TreeRootEntry, link: LinkEntry<K>) -> Self {
        Self {
            root,
            link,
            root_updated: Instant::now(),
            sync_state: SyncState::Pending,
            resolved_links: Default::default(),
            unresolved_links: Default::default(),
            unresolved_nodes: Default::default(),
            metrics: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn root(&self) -> &TreeRootEntry {
        &self.root
    }

    pub(crate) fn link(&self) -> &LinkEntry<K> {
        &self.link
    }

    pub(crate) fn resolved_links_mut(&mut self) -> &mut HashMap<String, LinkEntry<K>> {
        &mut self.resolved_links
    }

    pub(crate) fn extend_children(
        &mut self,
        kind: ResolveKind,
        children: impl IntoIterator<Item = String>,
    ) {
        match kind {
            ResolveKind::Enr => {
                self.unresolved_nodes.extend(children);
            }
            ResolveKind::Link => {
                self.unresolved_links.extend(children);
            }
        }
        self.update_metrics();
    }

    /// Advances the state of the tree by returning actions to perform
    pub(crate) fn poll(&mut self, now: Instant, update_timeout: Duration) -> Option<SyncAction> {
        self.update_metrics();
        match self.sync_state {
            SyncState::Pending => {
                self.sync_state = SyncState::Enr;
                return Some(SyncAction::Link(self.root.link_root.clone()))
            }
            SyncState::Enr => {
                self.sync_state = SyncState::Active;
                return Some(SyncAction::Enr(self.root.enr_root.clone()))
            }
            SyncState::Link => {
                self.sync_state = SyncState::Active;
                return Some(SyncAction::Link(self.root.link_root.clone()))
            }
            SyncState::Active => {
                if now > self.root_updated + update_timeout {
                    self.sync_state = SyncState::RootUpdate;
                    return Some(SyncAction::UpdateRoot)
                }
            }
            SyncState::RootUpdate => return None,
        }

        if let Some(link) = self.unresolved_links.pop_front() {
            return Some(SyncAction::Link(link))
        }

        let enr = self.unresolved_nodes.pop_front()?;
        Some(SyncAction::Enr(enr))
    }

    /// Updates the root and returns what changed
    pub(crate) fn update_root(&mut self, root: TreeRootEntry) {
        let enr = root.enr_root == self.root.enr_root;
        let link = root.link_root == self.root.link_root;

        self.root = root;
        self.root_updated = Instant::now();

        let state = match (enr, link) {
            (true, true) => {
                self.unresolved_nodes.clear();
                self.unresolved_links.clear();
                SyncState::Pending
            }
            (true, _) => {
                self.unresolved_nodes.clear();
                SyncState::Enr
            }
            (_, true) => {
                self.unresolved_links.clear();
                SyncState::Link
            }
            _ => {
                // unchanged
                return
            }
        };
        self.sync_state = state;
        self.update_metrics();
    }

    /// Updates metrics
    pub(crate) fn update_metrics(&self) {
        self.metrics.unresolved_nodes.set(self.unresolved_nodes.len() as f64);
        self.metrics.unresolved_links.set(self.unresolved_links.len() as f64);
        self.metrics.resolved_links.set(self.resolved_links.len() as f64);
    }
}

/// Represents synctree metrics
#[derive(Clone, Metrics)]
#[metrics(scope = "dns_disc")]
pub(crate) struct SyncTreeMetrics {
    /// The number of unresolved nodes
    pub(crate) unresolved_nodes: Gauge,
    /// The number of unresolved links
    pub(crate) unresolved_links: Gauge,
    /// The number of resolved links
    pub(crate) resolved_links: Gauge,
}

/// The action to perform by the service
pub(crate) enum SyncAction {
    UpdateRoot,
    Enr(String),
    Link(String),
}

/// How the [SyncTree::update_root] changed the root
enum SyncState {
    RootUpdate,
    Pending,
    Enr,
    Link,
    Active,
}

/// What kind of hash to resolve
pub(crate) enum ResolveKind {
    Enr,
    Link,
}

// === impl ResolveKind ===

impl ResolveKind {
    pub(crate) fn is_link(&self) -> bool {
        matches!(self, ResolveKind::Link)
    }
}
