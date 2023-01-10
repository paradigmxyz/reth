use crate::tree::{LinkEntry, TreeRootEntry};
use enr::EnrKeyUnambiguous;
use secp256k1::SecretKey;
use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
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
    /// Unresolved nodes of the tree
    missing_nodes: VecDeque<String>,
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
            missing_nodes: Default::default(),
        }
    }

    pub(crate) fn domain(&self) -> &str {
        &self.link.domain
    }

    pub(crate) fn link(&self) -> &LinkEntry<K> {
        &self.link
    }

    /// Advances the state of the tree by returning actions to perform
    pub(crate) fn poll(&mut self, now: Instant) -> Option<SyncAction> {
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
            SyncState::Active => {}
            SyncState::RootUpdate => return None,
        }

        let next = self.missing_nodes.pop_front()?;
        Some(SyncAction::Enr(next))
    }

    /// Updates the root and returns what changed
    pub(crate) fn update_root(&mut self, root: TreeRootEntry) {
        let enr = root.enr_root == self.root.enr_root;
        let link = root.link_root == self.root.link_root;

        self.root = root;
        self.root_updated = Instant::now();

        let state = match (enr, link) {
            (true, true) => {
                self.missing_nodes.clear();
                SyncState::Pending
            }
            (true, _) => {
                self.missing_nodes.clear();
                SyncState::Enr
            }
            (_, true) => SyncState::Link,
            _ => {
                // unchanged
                return
            }
        };
        self.sync_state = state;
    }
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
