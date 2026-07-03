use crate::sidecar::{
    check_sidecar_self_consistency, PartialStatelessSidecar, RootWitnessCompletenessReport,
    SerializableMultiProof, SidecarCheckError, StateTargetSet,
};
use alloy_consensus::Header;
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use reth_primitives_traits::Account;
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

type Result<T> = std::result::Result<T, SidecarWitnessCheckError>;

#[derive(Debug)]
pub struct MaterializedSidecarWitness {
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
    materialize_sidecar_witness_with_limits(sidecar, &SidecarWitnessCheckLimits::default())
}

pub fn materialize_sidecar_witness_with_limits(
    sidecar: &PartialStatelessSidecar,
    limits: &SidecarWitnessCheckLimits,
) -> Result<MaterializedSidecarWitness> {
    check_sidecar_witness_prefilter(sidecar, limits)?;

    let serializable_proof: SerializableMultiProof =
        bincode::deserialize(sidecar.witness.state.mpt_multiproof_bytes()).map_err(|err| {
            SidecarWitnessCheckError::Decode(format!("failed to decode sidecar multiproof: {err}"))
        })?;
    let multiproof = serializable_proof.to_multiproof();

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

        account_proof.verify(sidecar.parent_state_root).map_err(|err| {
            SidecarWitnessCheckError::Proof(format!(
                "invalid account/storage proof for {address:?}: {err}"
            ))
        })?;

        accounts.insert(address, account_proof.info);
        for proof in account_proof.storage_proofs {
            storage.insert((address, proof.key), proof.value);
        }
    }

    let codes = materialize_codes(sidecar)?;
    let headers = materialize_headers(sidecar)?;

    Ok(MaterializedSidecarWitness { accounts, storage, codes, headers })
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

pub fn root_witness_completeness_from_bundle(
    bundle_state: &BundleState,
    cache_miss_targets: &StateTargetSet,
) -> RootWitnessCompletenessReport {
    let mut account_paths = Vec::new();
    let mut storage_paths = Vec::new();

    for (address, account) in &bundle_state.state {
        if account.is_info_changed() || account.was_destroyed() {
            account_paths.push(*address);
        }
        for (slot, value) in &account.storage {
            if value.is_changed() {
                storage_paths.push((*address, B256::new(slot.to_be_bytes())));
            }
        }
    }

    root_witness_completeness_from_paths(account_paths, storage_paths, cache_miss_targets)
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
        sidecar::{
            CacheAnchor, PartialExecutionWitness, PartialExecutionWitnessState, WitnessTargets,
        },
        witness::WitnessResult,
    };
    use alloy_rlp::Encodable;

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
}
