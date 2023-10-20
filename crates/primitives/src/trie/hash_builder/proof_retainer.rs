use crate::{trie::Nibbles, Bytes};
use std::collections::BTreeMap;

/// Proof retainer is used to store proofs during merkle trie construction.
/// It is intended to be used within the [`HashBuilder`](crate::trie::HashBuilder).
#[derive(Debug)]
pub struct ProofRetainer {
    /// The nibbles of the target trie keys to retain proofs for.
    targets: Vec<Nibbles>,
    /// The map of retained proofs (RLP serialized trie nodes)
    /// with their corresponding key in the trie.
    proofs: BTreeMap<Nibbles, Bytes>,
}

impl ProofRetainer {
    /// Create new retainer with target nibbles.
    pub fn new(targets: Vec<Nibbles>) -> Self {
        Self { targets, proofs: Default::default() }
    }

    /// Returns `true` if the given prefix matches the retainer target.
    pub fn matches(&self, prefix: &Nibbles) -> bool {
        self.targets.iter().any(|target| target.starts_with(prefix))
    }

    /// Returns all collected proofs.
    pub fn into_proofs(self) -> BTreeMap<Nibbles, Bytes> {
        self.proofs
    }

    /// Retain the proof if the key matches any of the targets.
    pub fn retain(&mut self, prefix: &Nibbles, proof: &[u8]) {
        if self.matches(prefix) {
            self.proofs.insert(prefix.clone(), Bytes::from(proof.to_vec()));
        }
    }
}
