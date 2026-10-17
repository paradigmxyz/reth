use super::{
    add, evaluate, remaining_materialization, SelectionError, SelectionLimits, SelectionMultiproof,
    SelectionRequest, SelectionResponse,
};
use crate::{proof::hash_pair, proof_access::node_at};
use alloy_primitives::B256;
use reth_pureth_ssz::RetainedNode;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};

pub(crate) const PROOF_NODE_BYTES: usize = 640;

pub(crate) struct ProofReads<'a> {
    values: std::slice::Iter<'a, B256>,
    pub(crate) nodes: BTreeMap<u128, B256>,
}

impl<'a> ProofReads<'a> {
    pub(crate) fn new(
        proof: &'a SelectionMultiproof,
        limits: SelectionLimits,
    ) -> Result<Self, SelectionError> {
        proof_work(proof, limits)?;
        Ok(Self { values: proof.targets.iter(), nodes: BTreeMap::new() })
    }

    pub(crate) fn read(&mut self, index: u128) -> Result<B256, SelectionError> {
        if index == 0 {
            return Err(SelectionError::InvalidProof);
        }
        if let Some(node) = self.nodes.get(&index) {
            return Ok(*node);
        }
        let node = *self.values.next().ok_or(SelectionError::InvalidProof)?;
        self.nodes.insert(index, node);
        Ok(node)
    }

    pub(crate) fn finish(
        self,
        proof: &SelectionMultiproof,
        root: B256,
        limits: SelectionLimits,
    ) -> Result<(), SelectionError> {
        if self.values.len() != 0 {
            return Err(SelectionError::InvalidProof);
        }
        sparse(proof, &self.nodes, root, limits)?;
        Ok(())
    }
}

pub(crate) fn proof_work(
    proof: &SelectionMultiproof,
    limits: SelectionLimits,
) -> Result<usize, SelectionError> {
    let mut count = proof.targets.len();
    add(&mut count, proof.helpers.len(), limits.witness_hashes, "witness hashes")?;
    let bytes =
        count.checked_mul(PROOF_NODE_BYTES).ok_or(SelectionError::LimitExceeded("source bytes"))?;
    if bytes > limits.source_bytes {
        return Err(SelectionError::LimitExceeded("source bytes"));
    }
    Ok(bytes)
}

pub(crate) fn targets(
    indices: impl IntoIterator<Item = u128>,
    cancelled: &AtomicBool,
) -> Result<BTreeSet<u128>, SelectionError> {
    let mut result = indices.into_iter().collect::<BTreeSet<_>>();
    for index in result.clone() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        let mut parent = index >> 1;
        while parent != 0 {
            result.remove(&parent);
            parent >>= 1;
        }
    }
    Ok(result)
}

fn helpers(
    indices: &BTreeSet<u128>,
    limit: usize,
    limit_name: &'static str,
    cancelled: &AtomicBool,
) -> Result<BTreeSet<u128>, SelectionError> {
    let mut pending = indices.clone();
    let mut helpers = BTreeSet::new();
    while let Some(index) = pending.pop_last() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        if index == 0 {
            return Err(SelectionError::InvalidProof);
        }
        if index == 1 {
            break;
        }
        if !pending.remove(&(index ^ 1)) {
            helpers.insert(index ^ 1);
            if helpers.len() > limit {
                return Err(SelectionError::LimitExceeded(limit_name));
            }
        }
        pending.insert(index >> 1);
    }
    Ok(helpers)
}

pub(crate) fn build(
    tree: &RetainedNode,
    reads: &BTreeMap<u128, B256>,
    order: &[u128],
    limits: SelectionLimits,
    available: usize,
    cancelled: &AtomicBool,
) -> Result<SelectionMultiproof, SelectionError> {
    if reads.len().checked_mul(64).ok_or(SelectionError::LimitExceeded("source bytes"))? > available
    {
        return Err(SelectionError::LimitExceeded("source bytes"));
    }
    let indices = targets(reads.keys().copied(), cancelled)?;
    if reads.len() > limits.witness_hashes {
        return Err(SelectionError::LimitExceeded("witness hashes"));
    }
    let source_helpers = (available / PROOF_NODE_BYTES)
        .checked_sub(reads.len())
        .ok_or(SelectionError::LimitExceeded("source bytes"))?;
    let witness_helpers = limits.witness_hashes - reads.len();
    let (limit, limit_name) = if source_helpers < witness_helpers {
        (source_helpers, "source bytes")
    } else {
        (witness_helpers, "witness hashes")
    };
    let positions = helpers(&indices, limit, limit_name, cancelled)?;
    let count = reads
        .len()
        .checked_add(positions.len())
        .ok_or(SelectionError::LimitExceeded("witness hashes"))?;
    let bytes =
        count.checked_mul(PROOF_NODE_BYTES).ok_or(SelectionError::LimitExceeded("source bytes"))?;
    if bytes > available {
        return Err(SelectionError::LimitExceeded("source bytes"));
    }
    if order.len() != reads.len() ||
        order.iter().copied().collect::<BTreeSet<_>>() != reads.keys().copied().collect()
    {
        return Err(SelectionError::InvalidProof);
    }
    let targets = order.iter().map(|index| reads[index]).collect();
    let mut proof = Vec::with_capacity(positions.len());
    for index in positions.into_iter().rev() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SelectionError::Cancelled);
        }
        proof.push(node_at(tree, index).map_err(|_| SelectionError::InvalidProof)?);
    }
    Ok(SelectionMultiproof { targets, helpers: proof })
}

pub(crate) fn sparse(
    proof: &SelectionMultiproof,
    reads: &BTreeMap<u128, B256>,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<BTreeMap<u128, B256>, SelectionError> {
    proof_work(proof, limits)?;
    let cancelled = AtomicBool::new(false);
    let indices = targets(reads.keys().copied(), &cancelled)?;
    if indices.is_empty() || reads.contains_key(&0) || reads.len() != proof.targets.len() {
        return Err(SelectionError::InvalidProof);
    }
    let mut nodes = indices.iter().map(|index| (*index, reads[index])).collect::<BTreeMap<_, _>>();
    let positions = helpers(&indices, proof.helpers.len(), "witness hashes", &cancelled)?;
    if positions.len() != proof.helpers.len() {
        return Err(SelectionError::InvalidProof);
    }
    for (index, node) in positions.into_iter().rev().zip(&proof.helpers) {
        nodes.insert(index, *node);
    }
    let mut pending = nodes.keys().copied().collect::<BTreeSet<_>>();
    while let Some(index) = pending.pop_last() {
        if index == 1 {
            break;
        }
        let sibling = index ^ 1;
        if !pending.remove(&sibling) {
            return Err(SelectionError::InvalidProof);
        }
        let (left, right) = if index & 1 == 0 { (index, sibling) } else { (sibling, index) };
        let node = hash_pair(&nodes[&left], &nodes[&right]);
        nodes.insert(index >> 1, node);
        pending.insert(index >> 1);
    }
    if nodes.get(&1) != Some(&expected_root) {
        return Err(SelectionError::InvalidProof);
    }
    if reads.iter().any(|(index, node)| nodes.get(index) != Some(node)) {
        return Err(SelectionError::InvalidProof);
    }
    Ok(nodes)
}

pub(super) fn verify(
    request: &SelectionRequest,
    response: &SelectionResponse,
    expected_root: B256,
    limits: SelectionLimits,
) -> Result<(), SelectionError> {
    if response.results.iter().any(|result| result.witnesses.is_some()) {
        return Err(SelectionError::InvalidProof);
    }
    let proof = response.multiproof.as_ref().ok_or(SelectionError::InvalidProof)?;
    let mut reads = ProofReads::new(proof, limits)?;
    let mut expanded = 0;
    let mut matches = 0;
    let mut value_bytes = 0;
    for result in &response.results {
        for value in &result.values_ssz {
            add(&mut value_bytes, value.len(), limits.response_bytes, "value bytes")?;
        }
    }
    let remaining = SelectionLimits {
        source_bytes: remaining_materialization(
            proof_work(proof, limits)?,
            value_bytes,
            0,
            limits.source_bytes,
        )?,
        ..limits
    };
    for (selection, result) in request.selections.iter().zip(&response.results) {
        let mut read = |index| reads.read(index);
        let evaluated = evaluate(
            selection,
            &mut read,
            Some(&result.values_ssz),
            true,
            &mut expanded,
            &mut matches,
            remaining,
        )?;
        if evaluated.values_ssz != result.values_ssz || evaluated.matches != result.matches {
            return Err(SelectionError::InvalidValue);
        }
    }
    reads.finish(proof, expected_root, limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn shared_proofs_reconstruct_every_small_tree_selection() {
        let leaves =
            [0x11, 0x22, 0x33, 0x44].map(|byte| RetainedNode::leaf(B256::repeat_byte(byte)));
        let [a, b, c, d] = leaves;
        let tree = RetainedNode::pair(RetainedNode::pair(a, b), RetainedNode::pair(c, d));
        let limits = SelectionLimits::default();
        let cancelled = AtomicBool::new(false);
        for mask in 1_u8..128 {
            let reads = (1_u128..8)
                .filter(|index| mask & (1 << (*index - 1)) != 0)
                .map(|index| (index, node_at(&tree, index).unwrap()))
                .collect::<BTreeMap<_, _>>();
            let order = reads.keys().copied().collect::<Vec<_>>();
            let proof = build(&tree, &reads, &order, limits, usize::MAX, &cancelled).unwrap();
            let view = sparse(&proof, &reads, tree.root(), limits).unwrap();
            for (index, node) in reads {
                assert_eq!(view.get(&index), Some(&node), "selection mask {mask:#x}");
            }
        }
        let empty = SelectionMultiproof { targets: vec![], helpers: vec![] };
        assert_eq!(
            sparse(&empty, &BTreeMap::new(), tree.root(), limits),
            Err(SelectionError::InvalidProof)
        );
    }

    #[test]
    fn sparse_proofs_support_wide_internal_paths_without_wire_indices() {
        for (gindex, root) in [
            (
                (1_u128 << 64) | 1,
                b256!("567083eb829b83e1c488500af2ddbf3a403d437513f584c1bd45fcd6d9f0477e"),
            ),
            (
                1_u128 << 127,
                b256!("ead2852364d533d84a04f8722fe28bcc4867d06ac7d2643d46e383f07581a7a0"),
            ),
            (u128::MAX, b256!("df7a7d7c48cc4a30a81df99ccebc73c270cf16314a4587a584f4c5ec0788f659")),
        ] {
            let leaf = B256::repeat_byte(0x11);
            let mut tree = RetainedNode::leaf(leaf);
            for bit in 0..(u128::BITS - 1 - gindex.leading_zeros()) {
                tree = if (gindex >> bit) & 1 == 0 {
                    RetainedNode::pair(tree, RetainedNode::zero())
                } else {
                    RetainedNode::pair(RetainedNode::zero(), tree)
                };
            }
            let reads = BTreeMap::from([(gindex, leaf)]);
            let proof = build(
                &tree,
                &reads,
                &[gindex],
                SelectionLimits::default(),
                usize::MAX,
                &AtomicBool::new(false),
            )
            .unwrap();
            let value = serde_json::to_value(&proof).unwrap();
            assert_eq!(value["targets"][0], serde_json::to_value(leaf).unwrap());
            assert!(!value.to_string().contains("gindex"));
            let decoded: SelectionMultiproof = serde_json::from_value(value).unwrap();
            assert_eq!(decoded, proof);
            let view = sparse(&proof, &reads, root, SelectionLimits::default()).unwrap();
            assert_eq!(view[&gindex], leaf);
            assert_eq!(view[&1], root);
        }
    }

    #[test]
    fn canonical_targets_prune_ancestors_and_helper_work_cancels() {
        let cancelled = AtomicBool::new(false);
        assert_eq!(targets([1, 2, 4, 5, 5], &cancelled).unwrap(), BTreeSet::from([4, 5]));
        assert_eq!(
            helpers(&BTreeSet::from([4, 5]), 1, "witness hashes", &cancelled).unwrap(),
            BTreeSet::from([3])
        );
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(targets([4, 5], &cancelled), Err(SelectionError::Cancelled));
        assert_eq!(
            helpers(&BTreeSet::from([4, 5]), 1, "witness hashes", &cancelled),
            Err(SelectionError::Cancelled)
        );
        let proof = SelectionMultiproof { targets: vec![B256::ZERO, B256::ZERO], helpers: vec![] };
        assert_eq!(
            sparse(
                &proof,
                &BTreeMap::from([(0, B256::ZERO), (4, B256::ZERO)]),
                B256::ZERO,
                SelectionLimits::default()
            ),
            Err(SelectionError::InvalidProof)
        );
    }
}
