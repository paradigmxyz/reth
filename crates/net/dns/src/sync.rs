use crate::tree::{LinkEntry, TreeRootEntry};
use enr::EnrKeyUnambiguous;
use linked_hash_set::LinkedHashSet;
use secp256k1::SecretKey;
use std::time::{Duration, Instant};

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
    /// Unresolved links of the tree
    unresolved_links: LinkedHashSet<String>,
    /// Unresolved nodes of the tree
    unresolved_nodes: LinkedHashSet<String>,
}

// === impl SyncTree ===

impl<K: EnrKeyUnambiguous> SyncTree<K> {
    pub(crate) fn new(root: TreeRootEntry, link: LinkEntry<K>) -> Self {
        Self {
            root,
            link,
            root_updated: Instant::now(),
            sync_state: SyncState::Pending,
            unresolved_links: Default::default(),
            unresolved_nodes: Default::default(),
        }
    }

    #[cfg(test)]
    pub(crate) const fn root(&self) -> &TreeRootEntry {
        &self.root
    }

    pub(crate) const fn link(&self) -> &LinkEntry<K> {
        &self.link
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
    }

    /// Advances the state of the tree by returning actions to perform
    pub(crate) fn poll(&mut self, now: Instant, update_timeout: Duration) -> Option<SyncAction> {
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

    /// Records that the scheduled root lookup failed.
    ///
    /// Without this the tree would stay in [`SyncState::RootUpdate`] and stop polling entirely,
    /// so one failed lookup would take the tree out of rotation permanently. It retries at the
    /// next recheck instead.
    pub(crate) fn root_update_failed(&mut self) {
        // Only a lookup this tree scheduled releases it. Root lookups are also started from
        // bootstrap, from the public sync command and from link entries found in other trees, and
        // those outcomes must not overwrite a walk that is still recorded in `Pending`/`Enr`/
        // `Link`, or the tree would go quiet with its subtree unvisited.
        if matches!(self.sync_state, SyncState::RootUpdate) {
            self.root_updated = Instant::now();
            self.sync_state = SyncState::Active;
        }
    }

    /// Updates the root and returns what changed
    pub(crate) fn update_root(&mut self, root: TreeRootEntry) {
        let enr_unchanged = root.enr_root == self.root.enr_root;
        let link_unchanged = root.link_root == self.root.link_root;

        self.root = root;
        self.root_updated = Instant::now();

        let state = match (enr_unchanged, link_unchanged) {
            // Nothing to resync. The tree still has to leave `RootUpdate`, which would otherwise
            // park `poll` on `None` forever, but only when this answers its own recheck: an
            // unsolicited lookup of an unchanged root must leave a walk in progress alone.
            (true, true) => {
                if !matches!(self.sync_state, SyncState::RootUpdate) {
                    return
                }
                SyncState::Active
            }
            // only ENR changed
            (false, true) => {
                self.unresolved_nodes.clear();
                SyncState::Enr
            }
            // only LINK changed
            (true, false) => {
                self.unresolved_links.clear();
                SyncState::Link
            }
            // both changed
            (false, false) => {
                self.unresolved_nodes.clear();
                self.unresolved_links.clear();
                SyncState::Pending
            }
        };
        self.sync_state = state;
    }
}

/// The action to perform by the service
#[derive(Debug)]
pub(crate) enum SyncAction {
    UpdateRoot,
    Enr(String),
    Link(String),
}

/// How the [`SyncTree::update_root`] changed the root
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
    pub(crate) const fn is_link(&self) -> bool {
        matches!(self, Self::Link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enr::EnrKey;
    use secp256k1::rand::thread_rng;

    fn base_root() -> TreeRootEntry {
        // taken from existing tests to ensure valid formatting
        let s = "enrtree-root:v1 e=QFT4PBCRX4XQCV3VUYJ6BTCEPU l=JGUFMSAGI7KZYB3P7IZW4S5Y3A seq=3 sig=3FmXuVwpa8Y7OstZTx9PIb1mt8FrW7VpDOFv4AaGCsZ2EIHmhraWhe4NxYhQDlw5MjeFXYMbJjsPeKlHzmJREQE";
        s.parse::<TreeRootEntry>().unwrap()
    }

    fn make_tree() -> SyncTree {
        let secret_key = SecretKey::new(&mut thread_rng());
        let link =
            LinkEntry { domain: "nodes.example.org".to_string(), pubkey: secret_key.public() };
        SyncTree::new(base_root(), link)
    }

    fn advance_to_active(tree: &mut SyncTree) {
        // Move Pending -> (emit Link) -> Enr, then Enr -> (emit Enr) -> Active
        let now = Instant::now();
        let timeout = Duration::from_secs(60 * 60 * 24);
        let _ = tree.poll(now, timeout);
        let _ = tree.poll(now, timeout);
    }

    #[test]
    fn update_root_unchanged_no_action_from_active() {
        let mut tree = make_tree();
        let now = Instant::now();
        let timeout = Duration::from_secs(60 * 60 * 24);
        advance_to_active(&mut tree);

        // same root -> no resync
        let same = base_root();
        tree.update_root(same);
        assert!(tree.poll(now, timeout).is_none());
    }

    #[test]
    fn unsolicited_root_outcome_does_not_cancel_a_walk() {
        let timeout = Duration::from_secs(60);

        // a root lookup this tree did not schedule must not discard the walk it still owes. Root
        // lookups are also started from bootstrap, the public sync command and link entries in
        // other trees, and the query pool does not de-duplicate them.
        for unsolicited in [true, false] {
            let mut tree = make_tree();
            // Pending -> emits Link, leaving the enr root still to walk
            assert!(matches!(tree.poll(Instant::now(), timeout), Some(SyncAction::Link(_))));

            if unsolicited {
                tree.update_root(base_root());
            } else {
                tree.root_update_failed();
            }

            assert!(
                matches!(tree.poll(Instant::now(), timeout), Some(SyncAction::Enr(_))),
                "walk was cancelled by an unsolicited root outcome"
            );
        }
    }

    #[test]
    fn update_root_unchanged_still_rechecks_later() {
        let mut tree = make_tree();
        let timeout = Duration::from_secs(60);
        advance_to_active(&mut tree);

        // the recheck interval elapses and the root is looked up again
        let due = Instant::now() + timeout * 2;
        assert!(matches!(tree.poll(due, timeout), Some(SyncAction::UpdateRoot)));

        // it comes back unchanged, so there is nothing to resync right now
        tree.update_root(base_root());
        assert!(tree.poll(Instant::now(), timeout).is_none());

        // but the tree has to keep rechecking, otherwise it never picks up later changes
        let later = Instant::now() + timeout * 4;
        assert!(
            matches!(tree.poll(later, timeout), Some(SyncAction::UpdateRoot)),
            "tree stopped rechecking its root after an unchanged update"
        );
    }

    #[test]
    fn failed_root_update_still_rechecks_later() {
        let mut tree = make_tree();
        let timeout = Duration::from_secs(60);
        advance_to_active(&mut tree);

        let due = Instant::now() + timeout * 2;
        assert!(matches!(tree.poll(due, timeout), Some(SyncAction::UpdateRoot)));

        // the lookup failed, which must not take the tree out of rotation for good
        tree.root_update_failed();
        assert!(tree.poll(Instant::now(), timeout).is_none());

        let later = Instant::now() + timeout * 4;
        assert!(
            matches!(tree.poll(later, timeout), Some(SyncAction::UpdateRoot)),
            "tree stopped rechecking its root after a failed lookup"
        );
    }

    #[test]
    fn update_root_only_enr_changed_triggers_enr() {
        let mut tree = make_tree();
        advance_to_active(&mut tree);
        let mut new_root = base_root();
        new_root.enr_root = "NEW_ENR_ROOT".to_string();
        let now = Instant::now();
        let timeout = Duration::from_secs(60 * 60 * 24);

        tree.update_root(new_root.clone());
        match tree.poll(now, timeout) {
            Some(SyncAction::Enr(hash)) => assert_eq!(hash, new_root.enr_root),
            other => panic!("expected Enr action, got {:?}", other),
        }
    }

    #[test]
    fn update_root_only_link_changed_triggers_link() {
        let mut tree = make_tree();
        advance_to_active(&mut tree);
        let mut new_root = base_root();
        new_root.link_root = "NEW_LINK_ROOT".to_string();
        let now = Instant::now();
        let timeout = Duration::from_secs(60 * 60 * 24);

        tree.update_root(new_root.clone());
        match tree.poll(now, timeout) {
            Some(SyncAction::Link(hash)) => assert_eq!(hash, new_root.link_root),
            other => panic!("expected Link action, got {:?}", other),
        }
    }

    #[test]
    fn update_root_both_changed_triggers_link_then_enr() {
        let mut tree = make_tree();
        advance_to_active(&mut tree);
        let mut new_root = base_root();
        new_root.enr_root = "NEW_ENR_ROOT".to_string();
        new_root.link_root = "NEW_LINK_ROOT".to_string();
        let now = Instant::now();
        let timeout = Duration::from_secs(60 * 60 * 24);

        tree.update_root(new_root.clone());
        match tree.poll(now, timeout) {
            Some(SyncAction::Link(hash)) => assert_eq!(hash, new_root.link_root),
            other => panic!("expected first Link action, got {:?}", other),
        }
        match tree.poll(now, timeout) {
            Some(SyncAction::Enr(hash)) => assert_eq!(hash, new_root.enr_root),
            other => panic!("expected second Enr action, got {:?}", other),
        }
    }
}
