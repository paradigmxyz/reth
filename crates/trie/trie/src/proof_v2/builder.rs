use super::{node::*, DeferredValueEncoder, SubTrieTargets};
use crate::trie_cursor::depth_first;
use alloy_primitives::{keccak256, B256};
use alloy_rlp::Encodable;
use alloy_trie::TrieMask;
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{BranchNodeMasks, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};

/// Builds proof nodes from ordered leaves and disjoint branch hashes at absolute paths.
#[derive(Debug)]
pub(super) struct ProofBuilder<D> {
    children: Vec<ProofNode<D>>,
    retained_proofs: Vec<ProofTrieNodeV2>,
    rlp_nodes_bufs: Vec<Vec<RlpNode>>,
    rlp_encode_buf: Vec<u8>,
}

impl<D: DeferredValueEncoder> ProofBuilder<D> {
    pub(super) fn new() -> Self {
        Self {
            children: Vec::with_capacity(64),
            retained_proofs: Vec::with_capacity(32),
            rlp_nodes_bufs: Vec::with_capacity(8),
            rlp_encode_buf: Vec::with_capacity(1024),
        }
    }

    pub(super) fn clear(&mut self) {
        self.children.clear();
        self.retained_proofs.clear();
    }

    #[inline]
    pub(super) fn build(
        &mut self,
        targets: &SubTrieTargets<'_>,
        mut next: impl FnMut() -> Result<Option<ProofNode<D>>, StateProofError>,
    ) -> Result<(), StateProofError> {
        let first = next()?;
        let Some(first) = first else { return self.finish(targets, None) };
        let mut pending = next()?;
        if pending.is_none() {
            return self.finish(targets, Some(first))
        }
        let mut next_common = first.path.common_prefix_length(&pending.as_ref().unwrap().path);
        self.children.push(first);
        while pending.is_some() {
            self.subtree(&mut next, &mut pending, &mut next_common)?;
        }
        let root = self.children.pop();
        debug_assert!(pending.is_none());
        debug_assert!(self.children.is_empty());
        self.finish(targets, root)
    }

    /// Each call joins two or more children at their common prefix. A later item can wrap that
    /// branch in a shorter one.
    /// Paths stay absolute until their parent is known, so wrapping preserves all child paths.
    ///
    /// When this subtree ends, the pending input differs before the subtree path ends. The saved
    /// common prefix can therefore be used by the parent.
    ///
    /// Recursion advances at least one nibble and is bounded by the 64-nibble key length. Pending
    /// children share one reusable buffer; their values can finish work while later items arrive.
    fn subtree(
        &mut self,
        next: &mut impl FnMut() -> Result<Option<ProofNode<D>>, StateProofError>,
        pending: &mut Option<ProofNode<D>>,
        next_common: &mut usize,
    ) -> Result<(), StateProofError> {
        let common = *next_common;
        let first_path = self.children.last().unwrap().path;
        let next_path = pending.as_ref().unwrap().path;
        if common == first_path.len() || common == next_path.len() {
            return Err(StateProofError::TrieInconsistency(
                "proof inputs must have disjoint paths".to_string(),
            ))
        }
        let path = first_path.slice_unchecked(0, common);
        let start = self.children.len() - 1;
        let mut state_mask = TrieMask::new(1 << first_path.get_unchecked(common));

        loop {
            self.children.push(pending.take().unwrap());
            *pending = next()?;
            if let Some(item) = pending.as_ref() {
                *next_common = self.children.last().unwrap().path.common_prefix_length(&item.path);
            }
            while pending.is_some() && *next_common > common {
                self.subtree(next, pending, next_common)?;
            }
            let nibble = self.children.last().unwrap().path.get_unchecked(common);
            debug_assert!(!state_mask.is_bit_set(nibble));
            state_mask.set_bit(nibble);
            if pending.is_none() || *next_common < common {
                break
            }
        }
        let branch = self.branch(path, start, state_mask)?;
        self.children.push(branch);
        Ok(())
    }

    fn branch(
        &mut self,
        path: Nibbles,
        start: usize,
        state_mask: TrieMask,
    ) -> Result<ProofNode<D>, StateProofError> {
        let mut nodes = self.rlp_nodes_bufs.pop().unwrap_or_else(|| Vec::with_capacity(16));
        nodes.clear();
        let mut masks = BranchNodeMasks::default();
        let mut retain_depth = 0;

        for (nibble, child) in state_mask.iter().zip(self.children.drain(start..)) {
            retain_depth = retain_depth.max(child.retain_depth);
            let mut child_path = path;
            child_path.push_unchecked(nibble);
            let retain = child.retain_depth >= child_path.len();
            let (is_branch, tree) = match &child.kind {
                ProofNodeKind::Leaf(_) => (false, false),
                ProofNodeKind::Branch { masks, .. } => (true, !masks.is_empty()),
                ProofNodeKind::Hash { stored, .. } => (true, *stored),
            };
            let direct_branch = is_branch && child.path.len() == child_path.len();
            self.rlp_encode_buf.clear();

            let node = if let ProofNodeKind::Hash { hash, .. } = &child.kind &&
                direct_branch
            {
                RlpNode::word_rlp(hash)
            } else if retain && !matches!(child.kind, ProofNodeKind::Hash { .. }) {
                let proof = child.into_proof(child_path, &mut self.rlp_encode_buf)?;
                self.rlp_encode_buf.clear();
                proof.node.encode(&mut self.rlp_encode_buf);
                let node = RlpNode::from_rlp(&self.rlp_encode_buf);
                self.retained_proofs.push(proof);
                node
            } else {
                let (node, freed) = child.into_rlp(child_path.len(), &mut self.rlp_encode_buf)?;
                if let Some(buf) = freed {
                    self.rlp_nodes_bufs.push(buf);
                }
                node
            };
            masks.set_child_bits(nibble, direct_branch && node.is_hash(), tree);
            nodes.push(node);
        }
        Ok(ProofNode {
            path,
            kind: ProofNodeKind::Branch { children: nodes, state_mask, masks },
            retain_depth,
        })
    }

    fn finish(
        &mut self,
        targets: &SubTrieTargets<'_>,
        root: Option<ProofNode<D>>,
    ) -> Result<(), StateProofError> {
        let mut proof = if let Some(root) = root {
            self.rlp_encode_buf.clear();
            root.into_proof(targets.lower_bound, &mut self.rlp_encode_buf)?
        } else if targets.lower_bound.is_empty() {
            ProofTrieNodeV2::empty()
        } else {
            return Ok(())
        };
        // A direct root branch has no record in the branch table. An extension still carries
        // the masks of the branch below it.
        if targets.lower_bound.is_empty() &&
            matches!(&proof.node, TrieNodeV2::Branch(branch) if branch.key.is_empty())
        {
            proof.masks = None;
        }
        self.retained_proofs.push(proof);
        Ok(())
    }

    pub(super) fn take_proofs(&mut self) -> Vec<ProofTrieNodeV2> {
        self.retained_proofs.sort_unstable_by(|a, b| depth_first::cmp(&a.path, &b.path));
        self.retained_proofs.dedup_by(|a, b| a.path == b.path);
        core::mem::take(&mut self.retained_proofs)
    }

    pub(super) fn root_hash(&mut self, nodes: &[ProofTrieNodeV2]) -> Option<B256> {
        let root = nodes.iter().find(|node| node.path.is_empty())?;
        self.rlp_encode_buf.clear();
        root.node.encode(&mut self.rlp_encode_buf);
        Some(keccak256(&self.rlp_encode_buf))
    }
}
