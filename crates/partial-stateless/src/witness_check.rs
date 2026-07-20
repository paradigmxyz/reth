use crate::{
    sidecar::{
        check_sidecar_self_consistency, PartialStatelessSidecar, RootWitnessCompletenessReport,
        SerializableMultiProof, SidecarCheckError, StateTargetSet,
    },
    trie_cache::PartialTrieNodeCache,
};
use alloy_consensus::Header;
use alloy_primitives::{keccak256, map::B256Map, Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use reth_primitives_traits::Account;
use reth_trie_common::{
    BranchNodeMasksMap, BranchNodeV2, DecodedMultiProof, DecodedMultiProofV2, HashedPostState,
    KeccakKeyHasher, MultiProof, Nibbles, ProofTrieNodeV2, TrieNode, TrieNodeV2,
};
use reth_trie_sparse::{LeafUpdate, SparseTrie};
use revm_database::BundleState;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    error::Error,
    fmt,
};

#[derive(Debug, Clone)]
pub struct SidecarWitnessCheckLimits {
    pub max_accounts: usize,
    pub max_storage_slots: usize,
    pub max_code_hashes: usize,
    pub max_headers: usize,
    pub max_state_proof_bytes: usize,
    pub max_code_bytes: usize,
    pub max_header_bytes: usize,
    pub max_key_bytes: usize,
}

impl Default for SidecarWitnessCheckLimits {
    fn default() -> Self {
        Self {
            max_accounts: 100_000,
            max_storage_slots: 300_000,
            max_code_hashes: 20_000,
            max_headers: 256,
            max_state_proof_bytes: 64 * 1024 * 1024,
            max_code_bytes: 64 * 1024 * 1024,
            max_header_bytes: 2 * 1024 * 1024,
            max_key_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarWitnessCheckError {
    Sidecar(SidecarCheckError),
    LimitExceeded { label: &'static str, actual: usize, cap: usize },
    Decode(String),
    Proof(String),
    Bytecode(String),
    Header(String),
}

impl fmt::Display for SidecarWitnessCheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sidecar(err) => write!(f, "sidecar self-consistency failed: {err:?}"),
            Self::LimitExceeded { label, actual, cap } => {
                write!(f, "{label} exceeds witness check cap: actual={actual}, cap={cap}")
            }
            Self::Decode(err) => write!(f, "decode failed: {err}"),
            Self::Proof(err) => write!(f, "proof check failed: {err}"),
            Self::Bytecode(err) => write!(f, "bytecode check failed: {err}"),
            Self::Header(err) => write!(f, "header witness check failed: {err}"),
        }
    }
}

impl Error for SidecarWitnessCheckError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrieProofTarget {
    Account(B256),
    Storage { hashed_address: B256, hashed_slot: B256 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrieTransitionError {
    ProofRequired(Vec<TrieProofTarget>),
    Failed(String),
}

impl fmt::Display for TrieTransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProofRequired(targets) => {
                write!(f, "{} additional trie proofs required: {targets:?}", targets.len())
            }
            Self::Failed(err) => f.write_str(err),
        }
    }
}

impl Error for TrieTransitionError {}

type Result<T> = std::result::Result<T, SidecarWitnessCheckError>;

#[derive(Debug)]
pub struct MaterializedSidecarWitness {
    /// Self-contained parent-state miss proof, also revealed into the persistent sparse trie for
    /// trustless state-root computation (see [`compute_trustless_state_root`]).
    pub multiproof: MultiProof,
    pub accounts: HashMap<Address, Option<Account>>,
    pub storage: HashMap<(Address, B256), U256>,
    pub codes: HashMap<B256, Bytes>,
    pub headers: HashMap<u64, B256>,
}

pub fn check_sidecar_witness_prefilter(
    sidecar: &PartialStatelessSidecar,
    limits: &SidecarWitnessCheckLimits,
) -> Result<()> {
    check_sidecar_self_consistency(sidecar).map_err(SidecarWitnessCheckError::Sidecar)?;

    let targets = &sidecar.cache_miss_targets;
    ensure_cap("account targets", targets.accounts.len(), limits.max_accounts)?;
    ensure_cap("storage targets", targets.storage.len(), limits.max_storage_slots)?;
    ensure_cap("code targets", targets.code_hashes.len(), limits.max_code_hashes)?;
    ensure_cap("header witnesses", sidecar.witness.headers.len(), limits.max_headers)?;
    ensure_cap(
        "state proof bytes",
        sidecar.witness.state.mpt_multiproof_bytes().len(),
        limits.max_state_proof_bytes,
    )?;
    ensure_cap(
        "bytecode witness bytes",
        sidecar.witness.codes.iter().map(|bytes| bytes.len()).sum(),
        limits.max_code_bytes,
    )?;
    ensure_cap(
        "header witness bytes",
        sidecar.witness.headers.iter().map(|bytes| bytes.len()).sum(),
        limits.max_header_bytes,
    )?;
    ensure_cap(
        "key witness bytes",
        sidecar.witness.keys.iter().map(|bytes| bytes.len()).sum(),
        limits.max_key_bytes,
    )?;

    Ok(())
}

pub fn materialize_sidecar_witness(
    sidecar: &PartialStatelessSidecar,
) -> Result<MaterializedSidecarWitness> {
    // Parent-state miss proofs are self-contained, including for the standalone verifier.
    materialize_sidecar_witness_with_limits(sidecar, &SidecarWitnessCheckLimits::default())
}

pub fn materialize_sidecar_witness_with_limits(
    sidecar: &PartialStatelessSidecar,
    limits: &SidecarWitnessCheckLimits,
) -> Result<MaterializedSidecarWitness> {
    check_sidecar_witness_prefilter(sidecar, limits)?;

    let multiproof = decode_multiproof(sidecar.witness.state.mpt_multiproof_bytes())?;

    let mut grouped_targets: BTreeMap<Address, BTreeSet<B256>> = BTreeMap::new();
    for address in &sidecar.cache_miss_targets.accounts {
        grouped_targets.entry(*address).or_default();
    }
    for (address, slot) in &sidecar.cache_miss_targets.storage {
        grouped_targets.entry(*address).or_default().insert(*slot);
    }

    let mut accounts = HashMap::new();
    let mut storage = HashMap::new();
    for (address, slots) in grouped_targets {
        let slots = slots.into_iter().collect::<Vec<_>>();
        let account_proof = multiproof.account_proof(address, &slots).map_err(|err| {
            SidecarWitnessCheckError::Proof(format!(
                "failed to materialize account proof for {address:?}: {err}"
            ))
        })?;
        let account_proof =
            verify_and_trim_redundant_suffixes(account_proof, sidecar.parent_state_root).map_err(
                |err| {
                    SidecarWitnessCheckError::Proof(format!(
                        "invalid account/storage proof for {address:?}: {err}"
                    ))
                },
            )?;

        accounts.insert(address, account_proof.info);
        for proof in account_proof.storage_proofs {
            storage.insert((address, proof.key), proof.value);
        }
    }

    let codes = materialize_codes(sidecar)?;
    let headers = materialize_headers(sidecar)?;

    Ok(MaterializedSidecarWitness { multiproof, accounts, storage, codes, headers })
}

/// Removes proof nodes that occur after an inline trie node has already completed a lookup.
///
/// A multiproof can legitimately contain deeper nodes for a different structural target. The
/// legacy per-key extractor selects nodes by path prefix alone, so those nodes can appear as a
/// redundant suffix of an otherwise valid account or storage proof. A suffix is discarded only
/// when the remaining proof verifies against the authenticated root.
fn verify_and_trim_redundant_suffixes(
    mut account_proof: reth_trie_common::AccountProof,
    state_root: B256,
) -> std::result::Result<reth_trie_common::AccountProof, String> {
    for storage_proof in &mut account_proof.storage_proofs {
        while storage_proof.verify(account_proof.storage_root).is_err() &&
            !storage_proof.proof.is_empty()
        {
            storage_proof.proof.pop();
        }
        storage_proof.verify(account_proof.storage_root).map_err(|err| err.to_string())?;
    }

    while account_proof.verify(state_root).is_err() && !account_proof.proof.is_empty() {
        account_proof.proof.pop();
    }
    account_proof.verify(state_root).map_err(|err| err.to_string())?;
    Ok(account_proof)
}

/// Decode the local serialized multiproof representation carried in a sidecar.
fn decode_multiproof(bytes: &[u8]) -> Result<MultiProof> {
    let serializable: SerializableMultiProof = bincode::deserialize(bytes).map_err(|err| {
        SidecarWitnessCheckError::Decode(format!("failed to decode sidecar multiproof: {err}"))
    })?;
    Ok(serializable.to_multiproof())
}

/// Converts a legacy decoded proof to V2, requesting a deeper proof target when an exclusion
/// proof stops at a hashed extension child that the upstream sparse trie cannot reveal directly.
fn decoded_proof_nodes_to_v2_or_request(
    nodes: impl IntoIterator<Item = (Nibbles, TrieNode)>,
    masks: &BranchNodeMasksMap,
    storage_address: Option<B256>,
) -> std::result::Result<Vec<ProofTrieNodeV2>, TrieTransitionError> {
    let mut sorted = nodes.into_iter().collect::<Vec<_>>();
    sorted.sort_unstable_by(|a, b| reth_trie_common::depth_first_cmp(&a.0, &b.0));

    let mut result = Vec::with_capacity(sorted.len());
    for (path, node) in sorted {
        let masks = masks.get(&path).copied();
        match node {
            TrieNode::EmptyRoot => {
                result.push(ProofTrieNodeV2 { path, node: TrieNodeV2::EmptyRoot, masks });
            }
            TrieNode::Leaf(leaf) => {
                result.push(ProofTrieNodeV2 { path, node: TrieNodeV2::Leaf(leaf), masks });
            }
            TrieNode::Branch(branch) => {
                result.push(ProofTrieNodeV2 {
                    path,
                    node: TrieNodeV2::Branch(BranchNodeV2 {
                        key: Nibbles::new(),
                        branch_rlp_node: None,
                        stack: branch.stack,
                        state_mask: branch.state_mask,
                    }),
                    masks,
                });
            }
            TrieNode::Extension(ext) => {
                let expected_branch_path = path.join(&ext.key);
                let merged = result.last_mut().is_some_and(|last| {
                    if last.path != expected_branch_path {
                        return false
                    }
                    let TrieNodeV2::Branch(branch) = &mut last.node else { return false };
                    if !branch.key.is_empty() {
                        return false
                    }
                    branch.key = ext.key;
                    branch.branch_rlp_node = Some(ext.child.clone());
                    last.path = path;
                    true
                });
                if !merged {
                    if ext.child.is_hash() {
                        let child_path = path.join(&ext.key);
                        let mut target = [0u8; 32];
                        child_path.pack_to(&mut target);
                        let target = B256::from(target);
                        let proof_target = match storage_address {
                            Some(hashed_address) => {
                                TrieProofTarget::Storage { hashed_address, hashed_slot: target }
                            }
                            None => TrieProofTarget::Account(target),
                        };
                        return Err(TrieTransitionError::ProofRequired(vec![proof_target]))
                    } else {
                        let child_rlp = ext.child.clone();
                        let TrieNodeV2::Branch(mut branch) =
                            TrieNodeV2::decode(&mut ext.child.as_ref()).map_err(|err| {
                                TrieTransitionError::Failed(format!(
                                    "failed to decode inline extension child: {err}"
                                ))
                            })?
                        else {
                            return Err(TrieTransitionError::Failed(
                                "extension node child is not a branch".to_string(),
                            ))
                        };
                        branch.key = ext.key;
                        branch.branch_rlp_node = Some(child_rlp);
                        result.push(ProofTrieNodeV2 {
                            path,
                            node: TrieNodeV2::Branch(branch),
                            masks,
                        });
                    }
                }
            }
        }
    }
    Ok(result)
}

fn multiproof_to_v2_or_request(
    multiproof: MultiProof,
) -> std::result::Result<DecodedMultiProofV2, TrieTransitionError> {
    let decoded = DecodedMultiProof::try_from(multiproof).map_err(|err| {
        TrieTransitionError::Failed(format!("failed to decode parent multiproof: {err}"))
    })?;
    let account_proofs = decoded_proof_nodes_to_v2_or_request(
        decoded.account_subtree.into_inner(),
        &decoded.branch_node_masks,
        None,
    )?;
    let storage_proofs = decoded
        .storages
        .into_iter()
        .map(|(address, storage)| {
            Ok((
                address,
                decoded_proof_nodes_to_v2_or_request(
                    storage.subtree.into_inner(),
                    &storage.branch_node_masks,
                    Some(address),
                )?,
            ))
        })
        .collect::<std::result::Result<_, TrieTransitionError>>()?;
    Ok(DecodedMultiProofV2 { account_proofs, storage_proofs })
}

fn remove_leaf_and_collect_proofs(
    trie: &mut impl SparseTrie,
    key: B256,
) -> std::result::Result<Vec<B256>, String> {
    let mut updates = B256Map::default();
    updates.insert(key, LeafUpdate::Changed(Vec::new()));
    let mut proof_targets = Vec::new();
    trie.update_leaves(&mut updates, |target, _min_len| {
        proof_targets.push(target);
    })
    .map_err(|err| err.to_string())?;
    Ok(proof_targets)
}

fn dedup_proof_targets(targets: &mut Vec<TrieProofTarget>) {
    let mut seen = HashSet::with_capacity(targets.len());
    targets.retain(|target| seen.insert(*target));
}

/// Compute and persist the block post-state in the local sparse trie without a database walk.
///
/// The witness reveals parent-state paths for value-cache misses. Paths for cache hits are
/// already present in `trie_cache`. The execution diff updates both account and storage tries;
/// callers apply this to a cloned cache and commit the clone only after checking the block root.
pub fn compute_trustless_state_root(
    witness_multiproof: MultiProof,
    trie_cache: &mut PartialTrieNodeCache,
    bundle_state: &BundleState,
) -> Option<B256> {
    try_compute_trustless_state_root(witness_multiproof, trie_cache, bundle_state).ok()
}

/// Detailed variant of [`compute_trustless_state_root`] used by live diagnostics.
pub fn try_compute_trustless_state_root(
    witness_multiproof: MultiProof,
    trie_cache: &mut PartialTrieNodeCache,
    bundle_state: &BundleState,
) -> std::result::Result<B256, TrieTransitionError> {
    let decoded_multiproof = multiproof_to_v2_or_request(witness_multiproof)?;
    let sparse = trie_cache.sparse_mut();
    sparse.reveal_decoded_multiproof_v2(decoded_multiproof).map_err(|err| {
        TrieTransitionError::Failed(format!("failed to reveal parent multiproof: {err}"))
    })?;

    let hashed_post_state =
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(&bundle_state.state);

    // Storage is updated first. The account leaf is rewritten afterwards so its storage root
    // commits the new storage trie even when the account info itself did not change.
    let mut required_proofs = Vec::new();
    for (hashed_address, hashed_storage) in &hashed_post_state.storages {
        if hashed_storage.wiped {
            sparse.wipe_storage(*hashed_address).map_err(|err| {
                TrieTransitionError::Failed(format!(
                    "failed to wipe storage {hashed_address}: {err}"
                ))
            })?;
        }
        for (slot_hash, value) in &hashed_storage.storage {
            if *value == U256::ZERO {
                if hashed_storage.wiped {
                    continue
                }
                let storage_trie = sparse.storage_trie_mut(hashed_address).ok_or_else(|| {
                    TrieTransitionError::Failed(format!(
                        "storage trie was not revealed for address={hashed_address}"
                    ))
                })?;
                for hashed_slot in remove_leaf_and_collect_proofs(storage_trie, *slot_hash)
                    .map_err(|err| TrieTransitionError::Failed(format!(
                        "failed to remove storage path: address={hashed_address}, slot={slot_hash}: {err}"
                    )))?
                {
                    required_proofs.push(TrieProofTarget::Storage {
                        hashed_address: *hashed_address,
                        hashed_slot,
                    });
                }
            } else {
                sparse
                    .update_storage_leaf(
                        *hashed_address,
                        Nibbles::unpack(*slot_hash),
                        alloy_rlp::encode(value),
                    )
                    .map_err(|err| TrieTransitionError::Failed(format!(
                        "failed to update storage path: address={hashed_address}, slot={slot_hash}, value={value}, wiped={}: {err}",
                        hashed_storage.wiped
                    )))?;
            }
        }
    }
    dedup_proof_targets(&mut required_proofs);
    if !required_proofs.is_empty() {
        return Err(TrieTransitionError::ProofRequired(required_proofs))
    }

    let mut required_proofs = Vec::new();
    for (hashed_address, account) in &hashed_post_state.accounts {
        if account.is_none() {
            let account_trie = sparse
                .trie_mut()
                .as_revealed_mut()
                .ok_or_else(|| TrieTransitionError::Failed("account trie is blind".to_string()))?;
            for target in
                remove_leaf_and_collect_proofs(account_trie, *hashed_address).map_err(|err| {
                    TrieTransitionError::Failed(format!(
                        "failed to remove account path {hashed_address}: {err}"
                    ))
                })?
            {
                required_proofs.push(TrieProofTarget::Account(target));
            }
        } else {
            sparse.update_account_stateless(*hashed_address, *account).map_err(|err| {
                TrieTransitionError::Failed(format!(
                    "failed to update account path {hashed_address}, account={account:?}: {err}"
                ))
            })?;
        }
    }
    dedup_proof_targets(&mut required_proofs);
    if !required_proofs.is_empty() {
        return Err(TrieTransitionError::ProofRequired(required_proofs))
    }

    let mut required_proofs = Vec::new();
    for hashed_address in hashed_post_state.storages.keys() {
        if hashed_post_state.accounts.contains_key(hashed_address) {
            continue
        }
        let keep_account = sparse.update_account_storage_root(*hashed_address).map_err(|err| {
            TrieTransitionError::Failed(format!(
                "failed to propagate storage root for {hashed_address}: {err}"
            ))
        })?;
        if !keep_account {
            let account_trie = sparse
                .trie_mut()
                .as_revealed_mut()
                .ok_or_else(|| TrieTransitionError::Failed("account trie is blind".to_string()))?;
            for target in
                remove_leaf_and_collect_proofs(account_trie, *hashed_address).map_err(|err| {
                    TrieTransitionError::Failed(format!(
                        "failed to remove empty account {hashed_address}: {err}"
                    ))
                })?
            {
                required_proofs.push(TrieProofTarget::Account(target));
            }
        }
    }
    dedup_proof_targets(&mut required_proofs);
    if !required_proofs.is_empty() {
        return Err(TrieTransitionError::ProofRequired(required_proofs))
    }

    let state_root = sparse.root().map_err(|err| {
        TrieTransitionError::Failed(format!("failed to compute state root: {err}"))
    })?;
    drop(sparse.take_deferred_drops());
    trie_cache.set_state_root(state_root);
    Ok(state_root)
}

fn ensure_cap(label: &'static str, actual: usize, cap: usize) -> Result<()> {
    if actual > cap {
        return Err(SidecarWitnessCheckError::LimitExceeded { label, actual, cap });
    }
    Ok(())
}

fn materialize_codes(sidecar: &PartialStatelessSidecar) -> Result<HashMap<B256, Bytes>> {
    let declared: HashSet<B256> = sidecar.cache_miss_targets.code_hashes.iter().copied().collect();
    let mut codes = HashMap::new();
    for code in &sidecar.witness.codes {
        let code_hash = keccak256(code.as_ref());
        if !declared.contains(&code_hash) {
            return Err(SidecarWitnessCheckError::Bytecode(format!(
                "sidecar carries undeclared bytecode preimage: {code_hash:?}"
            )));
        }
        if codes.insert(code_hash, code.clone()).is_some() {
            return Err(SidecarWitnessCheckError::Bytecode(format!(
                "sidecar carries duplicate bytecode preimage: {code_hash:?}"
            )));
        }
    }
    if codes.len() != declared.len() {
        let missing = declared
            .into_iter()
            .filter(|code_hash| !codes.contains_key(code_hash))
            .collect::<Vec<_>>();
        return Err(SidecarWitnessCheckError::Bytecode(format!(
            "sidecar missing bytecode preimages: {missing:?}"
        )));
    }
    Ok(codes)
}

fn materialize_headers(sidecar: &PartialStatelessSidecar) -> Result<HashMap<u64, B256>> {
    let mut decoded = Vec::with_capacity(sidecar.witness.headers.len());
    let mut headers = HashMap::new();
    for raw in &sidecar.witness.headers {
        let mut raw = raw.as_ref();
        let header = Header::decode(&mut raw).map_err(|err| {
            SidecarWitnessCheckError::Header(format!(
                "failed to decode ancestor header witness: {err}"
            ))
        })?;
        if header.number >= sidecar.block_number {
            return Err(SidecarWitnessCheckError::Header(format!(
                "ancestor header witness is not an ancestor: number={}",
                header.number
            )));
        }
        let hash = header.hash_slow();
        if headers.insert(header.number, hash).is_some() {
            return Err(SidecarWitnessCheckError::Header(format!(
                "duplicate ancestor header witness: number={}",
                header.number
            )));
        }
        decoded.push((header.number, hash, header.parent_hash));
    }

    decoded.sort_by_key(|(number, _, _)| *number);
    if sidecar.block_number > 0 && !decoded.is_empty() {
        let Some(parent) = decoded.last() else { unreachable!("checked non-empty") };
        if parent.0 != sidecar.cache_block {
            return Err(SidecarWitnessCheckError::Header(format!(
                "ancestor header witness range must end at parent block: expected={}, got={}",
                sidecar.cache_block, parent.0
            )));
        }
        if parent.1 != sidecar.parent_hash {
            return Err(SidecarWitnessCheckError::Header(format!(
                "parent header witness hash mismatch: expected {:?}, got {:?}",
                sidecar.parent_hash, parent.1
            )));
        }
    }

    for pair in decoded.windows(2) {
        let [left, right] = pair else { continue };
        if left.0 + 1 != right.0 {
            return Err(SidecarWitnessCheckError::Header(format!(
                "ancestor header witness range has a gap: left={}, right={}",
                left.0, right.0
            )));
        }
        if right.2 != left.1 {
            return Err(SidecarWitnessCheckError::Header(format!(
                "ancestor header witness chain mismatch: child={}, parent_hash={:?}, expected={:?}",
                right.0, right.2, left.1
            )));
        }
    }

    Ok(headers)
}

/// Returns every parent-state account and storage path needed to apply a bundle diff.
pub fn root_witness_targets_from_bundle(bundle_state: &BundleState) -> StateTargetSet {
    let mut targets = StateTargetSet::default();
    for (address, account) in &bundle_state.state {
        if account.is_info_changed() || account.was_destroyed() {
            targets.accounts.push(*address);
        }
        for (slot, value) in &account.storage {
            if value.is_changed() {
                targets.storage.push((*address, B256::new(slot.to_be_bytes())));
            }
        }
    }
    targets.sort_dedup();
    targets
}

pub fn root_witness_completeness_from_bundle(
    bundle_state: &BundleState,
    cache_miss_targets: &StateTargetSet,
) -> RootWitnessCompletenessReport {
    let targets = root_witness_targets_from_bundle(bundle_state);
    root_witness_completeness_from_paths(targets.accounts, targets.storage, cache_miss_targets)
}

/// Compute root-witness readiness including paths already present in the pre-block trie cache.
pub fn root_witness_completeness_from_bundle_with_cache(
    bundle_state: &BundleState,
    cache_miss_targets: &StateTargetSet,
    trie_cache: &PartialTrieNodeCache,
) -> RootWitnessCompletenessReport {
    let mut report = root_witness_completeness_from_bundle(bundle_state, cache_miss_targets);

    let missing_accounts_before = report.missing_account_paths.len();
    let missing_storage_before = report.missing_storage_paths.len();
    report.missing_account_paths.retain(|address| !trie_cache.contains_account_path(address));
    report
        .missing_storage_paths
        .retain(|(address, slot)| !trie_cache.contains_storage_path(address, slot));
    report.covered_account_paths +=
        missing_accounts_before.saturating_sub(report.missing_account_paths.len());
    report.covered_storage_paths +=
        missing_storage_before.saturating_sub(report.missing_storage_paths.len());
    report.trustless_root_ready =
        report.missing_account_paths.is_empty() && report.missing_storage_paths.is_empty();
    report
}

pub fn root_witness_completeness_from_paths(
    account_paths: impl IntoIterator<Item = Address>,
    storage_paths: impl IntoIterator<Item = (Address, B256)>,
    cache_miss_targets: &StateTargetSet,
) -> RootWitnessCompletenessReport {
    let account_misses: HashSet<Address> = cache_miss_targets.accounts.iter().copied().collect();
    let account_paths_from_storage_miss: HashSet<Address> =
        cache_miss_targets.storage.iter().map(|(address, _)| *address).collect();
    let storage_misses: HashSet<(Address, B256)> =
        cache_miss_targets.storage.iter().copied().collect();

    let mut missing_account_paths = Vec::new();
    let mut covered_account_paths = 0usize;
    for address in account_paths {
        if account_misses.contains(&address) || account_paths_from_storage_miss.contains(&address) {
            covered_account_paths += 1;
        } else {
            missing_account_paths.push(address);
        }
    }
    missing_account_paths.sort();
    missing_account_paths.dedup();

    let mut missing_storage_paths = Vec::new();
    let mut covered_storage_paths = 0usize;
    for key in storage_paths {
        if storage_misses.contains(&key) {
            covered_storage_paths += 1;
        } else {
            missing_storage_paths.push(key);
        }
    }
    missing_storage_paths.sort();
    missing_storage_paths.dedup();

    RootWitnessCompletenessReport {
        trustless_root_ready: missing_account_paths.is_empty() && missing_storage_paths.is_empty(),
        missing_account_paths,
        missing_storage_paths,
        covered_account_paths,
        covered_storage_paths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        accessed_state::BlockAccessedState,
        network_cache::NetworkStateCache,
        policy::{AccountData, LastNBlocksPolicy},
        sidecar::{
            CacheAnchor, PartialExecutionWitness, PartialExecutionWitnessState, WitnessTargets,
        },
        witness::WitnessResult,
    };
    use alloy_rlp::Encodable;
    use reth_trie::HashBuilder;
    use reth_trie_common::{proof::ProofRetainer, StorageMultiProof, EMPTY_ROOT_HASH};
    use revm_database::{
        states::{StorageSlot, StorageWithOriginalValues},
        AccountStatus, BundleAccount,
    };
    use revm_state::AccountInfo;

    fn empty_stats() -> WitnessResult {
        WitnessResult {
            total_size_bytes: 0,
            account_proof_bytes: 0,
            storage_proof_bytes: 0,
            bytecode_bytes: 0,
            account_proof_nodes: 0,
            storage_proof_nodes: 0,
            target_accounts: 0,
            target_storage_slots: 0,
            computation_time_ms: None,
            cpu_time_ms: None,
            major_page_faults: None,
            minor_page_faults: None,
        }
    }

    fn test_anchor(block_number: u64, block_hash: B256, cache_policy_id: B256) -> CacheAnchor {
        CacheAnchor { block_number, block_hash, cache_policy_id, cache_root: keccak256(block_hash) }
    }

    fn header(number: u64, parent_hash: B256) -> Header {
        Header { number, parent_hash, gas_limit: 1, ..Default::default() }
    }

    fn encode_header(header: &Header) -> Bytes {
        let mut out = Vec::new();
        header.encode(&mut out);
        out.into()
    }

    fn sidecar_with_headers(
        block_number: u64,
        parent_hash: B256,
        headers: Vec<Bytes>,
    ) -> PartialStatelessSidecar {
        let cache_block = block_number.saturating_sub(1);
        let block_hash = B256::repeat_byte(0xbb);
        let cache_policy_id = B256::repeat_byte(0xcc);
        PartialStatelessSidecar {
            parent_hash,
            parent_state_root: B256::ZERO,
            block_hash,
            block_number,
            cache_block,
            cache_policy_id,
            prev_cache_anchor: test_anchor(cache_block, parent_hash, cache_policy_id),
            next_cache_anchor: test_anchor(block_number, block_hash, cache_policy_id),
            cache_policy_metadata: String::new(),
            cache_miss_targets: StateTargetSet::default(),
            witness_commitment: B256::ZERO,
            miss_manifest: WitnessTargets {
                missed_accounts: vec![],
                missed_storage: vec![],
                missed_code_hashes: vec![],
            },
            witness: PartialExecutionWitness {
                state: PartialExecutionWitnessState::MptMultiProof(vec![]),
                codes: vec![],
                keys: vec![],
                headers,
            },
            stats: empty_stats(),
        }
    }

    #[test]
    fn root_readiness_reports_cache_hit_account_write_as_missing() {
        let address = Address::repeat_byte(0x11);
        let report =
            root_witness_completeness_from_paths([address], [], &StateTargetSet::default());

        assert!(!report.trustless_root_ready);
        assert_eq!(report.missing_account_paths, vec![address]);
    }

    #[test]
    fn root_readiness_reports_cache_hit_storage_write_as_missing() {
        let address = Address::repeat_byte(0x11);
        let slot = B256::repeat_byte(0x22);
        let report =
            root_witness_completeness_from_paths([], [(address, slot)], &StateTargetSet::default());

        assert!(!report.trustless_root_ready);
        assert_eq!(report.missing_storage_paths, vec![(address, slot)]);
    }

    #[test]
    fn root_readiness_treats_miss_proof_paths_as_covered() {
        let address = Address::repeat_byte(0x11);
        let slot = B256::repeat_byte(0x22);
        let targets = StateTargetSet {
            accounts: vec![],
            storage: vec![(address, slot)],
            code_hashes: vec![],
        };
        let report = root_witness_completeness_from_paths([address], [(address, slot)], &targets);

        assert!(report.trustless_root_ready);
        assert!(report.missing_account_paths.is_empty());
        assert!(report.missing_storage_paths.is_empty());
        assert_eq!(report.covered_account_paths, 1);
        assert_eq!(report.covered_storage_paths, 1);
    }

    #[test]
    fn header_witness_allows_empty_when_blockhash_is_not_used() {
        let sidecar = sidecar_with_headers(11, B256::repeat_byte(0xaa), vec![]);
        let headers = materialize_headers(&sidecar).unwrap();

        assert!(headers.is_empty());
    }

    #[test]
    fn header_witness_rejects_unanchored_range() {
        let header_9 = header(9, B256::repeat_byte(0x09));
        let sidecar =
            sidecar_with_headers(11, B256::repeat_byte(0xaa), vec![encode_header(&header_9)]);

        assert!(matches!(
            materialize_headers(&sidecar),
            Err(SidecarWitnessCheckError::Header(err))
                if err.contains("must end at parent block")
        ));
    }

    #[test]
    fn header_witness_accepts_contiguous_chain_to_parent() {
        let header_9 = header(9, B256::repeat_byte(0x08));
        let header_10 = header(10, header_9.hash_slow());
        let parent_hash = header_10.hash_slow();
        let sidecar = sidecar_with_headers(
            11,
            parent_hash,
            vec![encode_header(&header_9), encode_header(&header_10)],
        );
        let headers = materialize_headers(&sidecar).unwrap();

        assert_eq!(headers.get(&9), Some(&header_9.hash_slow()));
        assert_eq!(headers.get(&10), Some(&parent_hash));
    }

    #[test]
    fn header_witness_rejects_chain_gaps() {
        let header_8 = header(8, B256::repeat_byte(0x07));
        let header_9 = header(9, header_8.hash_slow());
        let header_10 = header(10, header_9.hash_slow());
        let sidecar = sidecar_with_headers(
            11,
            header_10.hash_slow(),
            vec![encode_header(&header_8), encode_header(&header_10)],
        );

        assert!(matches!(
            materialize_headers(&sidecar),
            Err(SidecarWitnessCheckError::Header(err))
                if err.contains("range has a gap")
        ));
    }

    fn account_info(account: Account) -> AccountInfo {
        AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or_default(),
            code: None,
            ..Default::default()
        }
    }

    fn single_account_proof(
        address: Address,
        account: Account,
        storage: Option<(B256, U256)>,
    ) -> (MultiProof, B256) {
        let hashed_address = keccak256(address);
        let address_path = Nibbles::unpack(hashed_address);

        let (storage_root, storage_proof) = if let Some((slot, value)) = storage {
            let slot_path = Nibbles::unpack(keccak256(slot));
            let mut builder =
                HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([slot_path]));
            builder.add_leaf(slot_path, &alloy_rlp::encode(value));
            let root = builder.root();
            let subtree = builder.take_proof_nodes();
            (
                root,
                Some((
                    hashed_address,
                    StorageMultiProof { root, subtree, branch_node_masks: Default::default() },
                )),
            )
        } else {
            (EMPTY_ROOT_HASH, None)
        };

        let trie_account = account.into_trie_account(storage_root);
        let mut builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
        builder.add_leaf(address_path, &alloy_rlp::encode(trie_account));
        let state_root = builder.root();
        let account_subtree = builder.take_proof_nodes();

        let mut storages = alloy_primitives::map::B256Map::default();
        if let Some((address, proof)) = storage_proof {
            storages.insert(address, proof);
        }

        (
            MultiProof { account_subtree, branch_node_masks: Default::default(), storages },
            state_root,
        )
    }

    #[test]
    fn materialization_ignores_redundant_nodes_after_inline_storage_leaf() {
        let address = Address::repeat_byte(0x30);
        let slot = B256::repeat_byte(0x50);
        let value = U256::from(0x75);
        let account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x90)),
        };
        let (mut multiproof, state_root) =
            single_account_proof(address, account, Some((slot, value)));
        let storage = multiproof.storages.get_mut(&keccak256(address)).unwrap();
        let root_node = storage.subtree.get(&Nibbles::default()).unwrap().clone();
        let slot_path = Nibbles::unpack(keccak256(slot));
        storage.subtree.insert(slot_path.slice(0..1), root_node);

        let extracted = multiproof.account_proof(address, &[slot]).unwrap();
        assert!(extracted.verify(state_root).is_err());
        let trimmed = verify_and_trim_redundant_suffixes(extracted, state_root).unwrap();

        assert_eq!(trimmed.storage_proofs[0].value, value);
        assert_eq!(trimmed.storage_proofs[0].proof.len(), 1);
        trimmed.verify(state_root).unwrap();
    }

    #[test]
    fn local_sparse_trie_persists_updated_account_values() {
        let address = Address::repeat_byte(0x31);
        let code_hash = B256::repeat_byte(0x91);
        let parent = Account { nonce: 1, balance: U256::from(10), bytecode_hash: Some(code_hash) };
        let updated = Account { nonce: 2, balance: U256::from(20), bytecode_hash: Some(code_hash) };
        let (parent_proof, _) = single_account_proof(address, parent, None);

        let mut first_bundle = BundleState::default();
        first_bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(updated)),
                original_info: Some(account_info(parent)),
                storage: Default::default(),
                status: AccountStatus::Changed,
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let first_root =
            compute_trustless_state_root(parent_proof, &mut trie_cache, &first_bundle).unwrap();
        let (_, expected_first_root) = single_account_proof(address, updated, None);
        assert_eq!(first_root, expected_first_root);
        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            address,
            AccountData {
                nonce: updated.nonce,
                balance: updated.balance,
                code_hash: updated.bytecode_hash,
            },
        );
        values.on_block_executed(1, &accessed);
        trie_cache.retain_from_value_cache(&values);
        let metrics = trie_cache.validate_against_value_cache(&values).unwrap();
        assert_eq!(metrics.retained_account_paths, 1);
        assert_eq!(metrics.account_key_prefixes, [1; 6]);
        assert!(metrics.account_revealed_nodes > 0);

        // The next block needs no proof for this cache hit: it updates the same retained local
        // path.
        let updated_again =
            Account { nonce: 3, balance: U256::from(30), bytecode_hash: Some(code_hash) };
        let mut second_bundle = BundleState::default();
        second_bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(updated_again)),
                original_info: Some(account_info(updated)),
                storage: Default::default(),
                status: AccountStatus::Changed,
            },
        );
        let mut next_trie_cache = trie_cache.clone();
        let second_root = compute_trustless_state_root(
            MultiProof::default(),
            &mut next_trie_cache,
            &second_bundle,
        )
        .unwrap();
        let (_, expected_second_root) = single_account_proof(address, updated_again, None);
        assert_eq!(second_root, expected_second_root);
        assert_eq!(next_trie_cache.state_root(), Some(second_root));
        next_trie_cache.validate_against_value_cache(&values).unwrap();
    }

    #[test]
    fn cached_nonexistent_account_survives_retention() {
        let existing_address = Address::repeat_byte(0x32);
        let missing_address = Address::repeat_byte(0x33);
        let existing_account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x92)),
        };
        let missing_path = Nibbles::unpack(keccak256(missing_address));
        let existing_path = Nibbles::unpack(keccak256(existing_address));
        let mut builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([missing_path]));
        builder.add_leaf(
            existing_path,
            &alloy_rlp::encode(existing_account.into_trie_account(EMPTY_ROOT_HASH)),
        );
        let parent_root = builder.root();
        let proof = MultiProof {
            account_subtree: builder.take_proof_nodes(),
            branch_node_masks: Default::default(),
            storages: Default::default(),
        };

        let mut trie_cache = PartialTrieNodeCache::new();
        assert_eq!(
            compute_trustless_state_root(proof, &mut trie_cache, &BundleState::default()),
            Some(parent_root)
        );

        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            missing_address,
            AccountData { nonce: 0, balance: U256::ZERO, code_hash: None },
        );
        values.on_block_executed(1, &accessed);
        trie_cache.retain_from_value_cache(&values);

        trie_cache.validate_against_value_cache(&values).unwrap();
        assert!(trie_cache.contains_account_path(&missing_address));
        assert_eq!(trie_cache.state_root(), Some(parent_root));
    }

    #[test]
    fn exclusion_extension_requests_deeper_proof_before_storage_insertion() {
        let address = Address::repeat_byte(0x45);
        let account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x94)),
        };

        let slot_from_u64 = |value: u64| {
            let mut bytes = [0u8; 32];
            bytes[24..].copy_from_slice(&value.to_be_bytes());
            B256::from(bytes)
        };
        let mut first_by_prefix = [None; 256];
        let mut existing_slots = None;
        for value in 0..10_000 {
            let slot = slot_from_u64(value);
            let path = Nibbles::unpack(keccak256(slot));
            let first = path.get_unchecked(0);
            let second = path.get_unchecked(1);
            let prefix = (first as usize) << 4 | second as usize;
            if let Some(previous) = first_by_prefix[prefix] {
                existing_slots = Some((first, second, previous, slot));
                break
            }
            first_by_prefix[prefix] = Some(slot);
        }
        let (existing_first, existing_second, slot_a, slot_b) =
            existing_slots.expect("find colliding two-nibble prefix");
        let slot_c = (10_000..20_000)
            .map(slot_from_u64)
            .find(|slot| Nibbles::unpack(keccak256(slot)).get_unchecked(0) != existing_first)
            .expect("find root-branch sibling");
        let target_slot = (10_000..20_000)
            .map(slot_from_u64)
            .find(|slot| {
                let path = Nibbles::unpack(keccak256(slot));
                path.get_unchecked(0) == existing_first && path.get_unchecked(1) != existing_second
            })
            .expect("find target diverging inside non-root extension");
        let target_path = Nibbles::unpack(keccak256(target_slot));

        let old_a = U256::from(3);
        let old_b = U256::from(4);
        let old_c = U256::from(6);
        let inserted = U256::from(5);
        let mut existing = vec![
            (Nibbles::unpack(keccak256(slot_a)), old_a),
            (Nibbles::unpack(keccak256(slot_b)), old_b),
            (Nibbles::unpack(keccak256(slot_c)), old_c),
        ];
        existing.sort_unstable_by_key(|(path, _)| *path);

        let mut storage_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([target_path]));
        for (path, value) in &existing {
            storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        let parent_storage_root = storage_builder.root();
        let storage_subtree = storage_builder.take_proof_nodes();
        let extension_node = storage_subtree
            .iter()
            .find_map(|(path, bytes)| {
                (path == &Nibbles::from_nibbles([existing_first])).then_some(bytes)
            })
            .expect("non-root extension proof node");
        assert!(matches!(
            TrieNode::decode(&mut extension_node.as_ref()).unwrap(),
            TrieNode::Extension(ext) if ext.child.is_hash()
        ));

        let hashed_address = keccak256(address);
        let address_path = Nibbles::unpack(hashed_address);
        let mut account_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
        account_builder.add_leaf(
            address_path,
            &alloy_rlp::encode(account.into_trie_account(parent_storage_root)),
        );
        let parent_state_root = account_builder.root();
        let account_subtree = account_builder.take_proof_nodes();
        let storage_proof = StorageMultiProof {
            root: parent_storage_root,
            subtree: storage_subtree,
            branch_node_masks: Default::default(),
        };
        let mut storages = alloy_primitives::map::B256Map::default();
        storages.insert(hashed_address, storage_proof);
        let mut parent_proof =
            MultiProof { account_subtree, branch_node_masks: Default::default(), storages };

        let requested_slot = match multiproof_to_v2_or_request(parent_proof.clone()) {
            Err(TrieTransitionError::ProofRequired(targets)) => match targets.as_slice() {
                [TrieProofTarget::Storage { hashed_address: requested_address, hashed_slot }] => {
                    assert_eq!(*requested_address, hashed_address);
                    *hashed_slot
                }
                other => panic!("expected one deeper storage proof request, got {other:?}"),
            },
            other => panic!("expected deeper storage proof request, got {other:?}"),
        };
        let requested_path = Nibbles::unpack(requested_slot);
        assert!(
            requested_path.starts_with(&Nibbles::from_nibbles([existing_first, existing_second,]))
        );

        let mut enriched_storage_builder = HashBuilder::default()
            .with_proof_retainer(ProofRetainer::from_iter([target_path, requested_path]));
        for (path, value) in &existing {
            enriched_storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        assert_eq!(enriched_storage_builder.root(), parent_storage_root);
        parent_proof.storages.insert(
            hashed_address,
            StorageMultiProof {
                root: parent_storage_root,
                subtree: enriched_storage_builder.take_proof_nodes(),
                branch_node_masks: Default::default(),
            },
        );

        // A zero value is an exclusion witness rather than a leaf. Per-block retention must keep
        // the non-root extension that proves this cached lookup is absent.
        let mut exclusion_cache = PartialTrieNodeCache::new();
        let exclusion_root = compute_trustless_state_root(
            parent_proof.clone(),
            &mut exclusion_cache,
            &BundleState::default(),
        )
        .unwrap();
        assert_eq!(exclusion_root, parent_state_root);
        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            address,
            AccountData {
                nonce: account.nonce,
                balance: account.balance,
                code_hash: account.bytecode_hash,
            },
        );
        accessed.storage.insert((address, target_slot), U256::ZERO);
        values.on_block_executed(1, &accessed);
        exclusion_cache.retain_from_value_cache(&values);
        exclusion_cache.validate_against_value_cache(&values).unwrap();

        let mut storage = StorageWithOriginalValues::default();
        storage.insert(
            U256::from_be_slice(target_slot.as_slice()),
            StorageSlot { previous_or_original_value: U256::ZERO, present_value: inserted },
        );
        let mut bundle = BundleState::default();
        bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(account)),
                original_info: Some(account_info(account)),
                storage,
                status: AccountStatus::Changed,
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let actual_root =
            compute_trustless_state_root(parent_proof, &mut trie_cache, &bundle).unwrap();

        existing.push((target_path, inserted));
        existing.sort_unstable_by_key(|(path, _)| *path);
        let mut expected_storage_builder = HashBuilder::default();
        for (path, value) in &existing {
            expected_storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        let expected_storage_root = expected_storage_builder.root();
        let mut expected_account_builder = HashBuilder::default();
        expected_account_builder.add_leaf(
            address_path,
            &alloy_rlp::encode(account.into_trie_account(expected_storage_root)),
        );
        let expected_state_root = expected_account_builder.root();
        assert_eq!(actual_root, expected_state_root);

        assert_eq!(trie_cache.state_root(), Some(expected_state_root));
    }

    #[test]
    fn storage_deletion_requests_blinded_sibling_and_retries() {
        let address = Address::repeat_byte(0x46);
        let account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x95)),
        };
        let slot_a = B256::repeat_byte(0x51);
        let slot_b = B256::repeat_byte(0x52);
        let path_a = Nibbles::unpack(keccak256(slot_a));
        let path_b = Nibbles::unpack(keccak256(slot_b));
        let old_a = U256::from(7);
        let old_b = U256::from(9);
        let mut parent_storage = vec![(path_a, old_a), (path_b, old_b)];
        parent_storage.sort_unstable_by_key(|(path, _)| *path);

        let mut storage_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([path_a]));
        for (path, value) in &parent_storage {
            storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        let parent_storage_root = storage_builder.root();
        let hashed_address = keccak256(address);
        let address_path = Nibbles::unpack(hashed_address);
        let mut account_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
        account_builder.add_leaf(
            address_path,
            &alloy_rlp::encode(account.into_trie_account(parent_storage_root)),
        );
        let _parent_state_root = account_builder.root();
        let mut storages = B256Map::default();
        storages.insert(
            hashed_address,
            StorageMultiProof {
                root: parent_storage_root,
                subtree: storage_builder.take_proof_nodes(),
                branch_node_masks: Default::default(),
            },
        );
        let parent_proof = MultiProof {
            account_subtree: account_builder.take_proof_nodes(),
            branch_node_masks: Default::default(),
            storages,
        };

        let mut storage = StorageWithOriginalValues::default();
        storage.insert(
            U256::from_be_slice(slot_a.as_slice()),
            StorageSlot { previous_or_original_value: old_a, present_value: U256::ZERO },
        );
        let mut bundle = BundleState::default();
        bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(account)),
                original_info: Some(account_info(account)),
                storage,
                status: AccountStatus::Changed,
            },
        );

        let mut failed_attempt = PartialTrieNodeCache::new();
        let requested_slot = match try_compute_trustless_state_root(
            parent_proof.clone(),
            &mut failed_attempt,
            &bundle,
        ) {
            Err(TrieTransitionError::ProofRequired(targets)) => match targets.as_slice() {
                [TrieProofTarget::Storage { hashed_address: requested_address, hashed_slot }] => {
                    assert_eq!(*requested_address, hashed_address);
                    *hashed_slot
                }
                other => panic!("expected one blinded sibling proof request, got {other:?}"),
            },
            other => panic!("expected blinded sibling proof request, got {other:?}"),
        };

        let mut enriched_storage_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([
                path_a,
                Nibbles::unpack(requested_slot),
            ]));
        for (path, value) in &parent_storage {
            enriched_storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        assert_eq!(enriched_storage_builder.root(), parent_storage_root);
        let mut enriched_proof = parent_proof;
        enriched_proof.storages.insert(
            hashed_address,
            StorageMultiProof {
                root: parent_storage_root,
                subtree: enriched_storage_builder.take_proof_nodes(),
                branch_node_masks: Default::default(),
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let actual_root =
            try_compute_trustless_state_root(enriched_proof, &mut trie_cache, &bundle).unwrap();
        let mut expected_storage_builder = HashBuilder::default();
        expected_storage_builder.add_leaf(path_b, &alloy_rlp::encode(old_b));
        let expected_storage_root = expected_storage_builder.root();
        let mut expected_account_builder = HashBuilder::default();
        expected_account_builder.add_leaf(
            address_path,
            &alloy_rlp::encode(account.into_trie_account(expected_storage_root)),
        );
        let expected_state_root = expected_account_builder.root();
        assert_eq!(actual_root, expected_state_root);

        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            address,
            AccountData {
                nonce: account.nonce,
                balance: account.balance,
                code_hash: account.bytecode_hash,
            },
        );
        accessed.storage.insert((address, slot_a), U256::ZERO);
        values.on_block_executed(1, &accessed);
        trie_cache.retain_from_value_cache(&values);
        trie_cache.validate_against_value_cache(&values).unwrap();
        assert!(trie_cache.contains_storage_path(&address, &slot_a));
    }

    #[test]
    fn independent_storage_deletions_request_proofs_as_one_batch() {
        let address = Address::repeat_byte(0x47);
        let account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x97)),
        };
        let slot_from_u64 = |value: u64| {
            let mut bytes = [0u8; 32];
            bytes[24..].copy_from_slice(&value.to_be_bytes());
            B256::from(bytes)
        };
        let mut slots_by_first: [Vec<B256>; 16] = std::array::from_fn(|_| Vec::new());
        for value in 0..10_000 {
            let slot = slot_from_u64(value);
            let first = Nibbles::unpack(keccak256(slot)).get_unchecked(0) as usize;
            if slots_by_first[first].len() < 2 {
                slots_by_first[first].push(slot);
            }
            if slots_by_first.iter().filter(|slots| slots.len() == 2).count() >= 2 {
                break
            }
        }
        let pairs =
            slots_by_first.into_iter().filter(|slots| slots.len() == 2).take(2).collect::<Vec<_>>();
        assert_eq!(pairs.len(), 2);
        let deleted_slots = [pairs[0][0], pairs[1][0]];
        let all_slots = [pairs[0][0], pairs[0][1], pairs[1][0], pairs[1][1]];
        let deleted_paths = deleted_slots.map(|slot| Nibbles::unpack(keccak256(slot)));

        let mut parent_storage = all_slots
            .into_iter()
            .enumerate()
            .map(|(index, slot)| (Nibbles::unpack(keccak256(slot)), U256::from(index + 1)))
            .collect::<Vec<_>>();
        parent_storage.sort_unstable_by_key(|(path, _)| *path);
        let mut storage_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter(deleted_paths));
        for (path, value) in &parent_storage {
            storage_builder.add_leaf(*path, &alloy_rlp::encode(value));
        }
        let parent_storage_root = storage_builder.root();
        let hashed_address = keccak256(address);
        let address_path = Nibbles::unpack(hashed_address);
        let mut account_builder =
            HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([address_path]));
        account_builder.add_leaf(
            address_path,
            &alloy_rlp::encode(account.into_trie_account(parent_storage_root)),
        );
        account_builder.root();
        let mut storages = B256Map::default();
        storages.insert(
            hashed_address,
            StorageMultiProof {
                root: parent_storage_root,
                subtree: storage_builder.take_proof_nodes(),
                branch_node_masks: Default::default(),
            },
        );
        let proof = MultiProof {
            account_subtree: account_builder.take_proof_nodes(),
            branch_node_masks: Default::default(),
            storages,
        };

        let mut storage = StorageWithOriginalValues::default();
        for deleted_slot in deleted_slots {
            let previous = parent_storage
                .iter()
                .find_map(|(path, value)| {
                    (*path == Nibbles::unpack(keccak256(deleted_slot))).then_some(*value)
                })
                .unwrap();
            storage.insert(
                U256::from_be_slice(deleted_slot.as_slice()),
                StorageSlot { previous_or_original_value: previous, present_value: U256::ZERO },
            );
        }
        let mut bundle = BundleState::default();
        bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(account)),
                original_info: Some(account_info(account)),
                storage,
                status: AccountStatus::Changed,
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let targets = match try_compute_trustless_state_root(proof, &mut trie_cache, &bundle) {
            Err(TrieTransitionError::ProofRequired(targets)) => targets,
            other => panic!("expected a batched proof request, got {other:?}"),
        };
        assert!(targets.len() >= 2, "expected independent deletions to be batched: {targets:?}");
        assert!(targets.iter().all(|target| matches!(
            target,
            TrieProofTarget::Storage { hashed_address: target_address, .. }
                if *target_address == hashed_address
        )));
    }

    #[test]
    fn storage_only_change_updates_account_storage_root() {
        let address = Address::repeat_byte(0x41);
        let slot = B256::repeat_byte(0x52);
        let code_hash = B256::repeat_byte(0x93);
        let account = Account { nonce: 1, balance: U256::from(10), bytecode_hash: Some(code_hash) };
        let old_value = U256::from(7);
        let new_value = U256::from(9);
        let (parent_proof, _) = single_account_proof(address, account, Some((slot, old_value)));

        let mut storage = StorageWithOriginalValues::default();
        storage.insert(
            U256::from_be_slice(slot.as_slice()),
            StorageSlot { previous_or_original_value: old_value, present_value: new_value },
        );
        let mut bundle = BundleState::default();
        bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(account)),
                original_info: Some(account_info(account)),
                storage,
                status: AccountStatus::Changed,
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let root = compute_trustless_state_root(parent_proof, &mut trie_cache, &bundle).unwrap();
        let (_, expected_root) = single_account_proof(address, account, Some((slot, new_value)));
        assert_eq!(root, expected_root);

        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(60)),
            Box::new(LastNBlocksPolicy::new(30)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            address,
            AccountData {
                nonce: account.nonce,
                balance: account.balance,
                code_hash: account.bytecode_hash,
            },
        );
        accessed.storage.insert((address, slot), new_value);
        values.on_block_executed(1, &accessed);
        trie_cache.retain_from_value_cache(&values);
        let metrics = trie_cache.validate_against_value_cache(&values).unwrap();
        assert_eq!(metrics.retained_account_paths, 1);
        assert_eq!(metrics.retained_storage_tries, 1);
        assert_eq!(metrics.retained_storage_paths, 1);
        assert!(metrics.storage_revealed_nodes > 0);

        let newer_value = U256::from(11);
        let mut newer_storage = StorageWithOriginalValues::default();
        newer_storage.insert(
            U256::from_be_slice(slot.as_slice()),
            StorageSlot { previous_or_original_value: new_value, present_value: newer_value },
        );
        let mut next_bundle = BundleState::default();
        next_bundle.state.insert(
            address,
            BundleAccount {
                info: Some(account_info(account)),
                original_info: Some(account_info(account)),
                storage: newer_storage,
                status: AccountStatus::Changed,
            },
        );
        let mut next_trie_cache = trie_cache.clone();
        let next_root =
            compute_trustless_state_root(MultiProof::default(), &mut next_trie_cache, &next_bundle)
                .unwrap();
        let (_, expected_next_root) =
            single_account_proof(address, account, Some((slot, newer_value)));
        assert_eq!(next_root, expected_next_root);
        assert_eq!(next_trie_cache.state_root(), Some(next_root));
        next_trie_cache.validate_against_value_cache(&values).unwrap();
    }

    #[test]
    fn sparse_membership_follows_account_and_storage_windows() {
        let address = Address::repeat_byte(0x42);
        let slot = B256::repeat_byte(0x53);
        let account = Account {
            nonce: 1,
            balance: U256::from(10),
            bytecode_hash: Some(B256::repeat_byte(0x96)),
        };
        let value = U256::from(7);
        let (parent_proof, parent_root) =
            single_account_proof(address, account, Some((slot, value)));
        let mut trie_cache = PartialTrieNodeCache::new();
        assert_eq!(
            compute_trustless_state_root(parent_proof, &mut trie_cache, &BundleState::default()),
            Some(parent_root)
        );

        let mut values = NetworkStateCache::new(
            Box::new(LastNBlocksPolicy::new(3)),
            Box::new(LastNBlocksPolicy::new(1)),
        );
        let mut accessed = BlockAccessedState::default();
        accessed.accounts.insert(
            address,
            AccountData {
                nonce: account.nonce,
                balance: account.balance,
                code_hash: account.bytecode_hash,
            },
        );
        accessed.storage.insert((address, slot), value);
        values.on_block_executed(10, &accessed);
        trie_cache.retain_from_value_cache(&values);
        trie_cache.validate_against_value_cache(&values).unwrap();
        assert!(trie_cache.has_storage_trie(&address));

        let anchor_before =
            values.cache_anchor(10, B256::repeat_byte(0xa1), B256::repeat_byte(0xb1));
        trie_cache.retain_from_value_cache(&values);
        assert_eq!(
            values.cache_anchor(10, B256::repeat_byte(0xa1), B256::repeat_byte(0xb1)),
            anchor_before
        );

        values.on_block_executed(12, &BlockAccessedState::default());
        trie_cache.retain_from_value_cache(&values);
        let storage_evicted = trie_cache.validate_against_value_cache(&values).unwrap();
        assert_eq!(storage_evicted.retained_account_paths, 1);
        assert_eq!(storage_evicted.retained_storage_paths, 0);
        assert!(!trie_cache.has_storage_trie(&address));

        values.on_block_executed(14, &BlockAccessedState::default());
        trie_cache.retain_from_value_cache(&values);
        let all_evicted = trie_cache.validate_against_value_cache(&values).unwrap();
        assert_eq!(all_evicted.retained_account_paths, 0);
        assert_eq!(all_evicted.retained_storage_paths, 0);
        assert_eq!(trie_cache.state_root(), Some(parent_root));
    }

    #[test]
    fn trustless_root_is_none_when_trie_is_blind() {
        // With no witness and no cached trie nodes, the account trie is never revealed, so a
        // trustless root cannot be produced — the function must report `None`, not panic.
        let mut trie_cache = PartialTrieNodeCache::new();
        let root = compute_trustless_state_root(
            MultiProof::default(),
            &mut trie_cache,
            &BundleState::default(),
        );
        assert_eq!(root, None);
    }

    #[test]
    fn trustless_root_is_none_when_witness_and_cache_cannot_cover_diff() {
        // A bundle that changes an account, but neither the witness nor the trie cache reveals its
        // path → blind → `None` (never a spurious `Some`).
        use revm_state::AccountInfo;

        let mut bundle = BundleState::default();
        bundle.state.insert(
            Address::repeat_byte(0x42),
            revm_database::BundleAccount {
                info: Some(AccountInfo {
                    balance: U256::from(1u64),
                    nonce: 1,
                    ..Default::default()
                }),
                original_info: None,
                storage: Default::default(),
                status: revm_database::AccountStatus::Changed,
            },
        );

        let mut trie_cache = PartialTrieNodeCache::new();
        let root = compute_trustless_state_root(MultiProof::default(), &mut trie_cache, &bundle);
        assert_eq!(root, None);
    }
}
