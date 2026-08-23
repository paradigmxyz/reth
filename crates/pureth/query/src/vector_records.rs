use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    address_target_node, branch_positions, parse_path, receipt_log_address_gindex, resolve_v0,
    validate_runtime_bounds,
    vector::{decode_fixed, validate_receipt_fixture, ReceiptFixture, VectorError},
    SCHEMA_DIGEST, SCHEMA_ID,
};

const FORMAT_VERSION: &str = "pureth_receipt_proof_vectors_v0";
const OBJECT_KIND: &str = "receipts";
const PROOF_FORMAT: &str = "merkle_branch_v0";
const ROOT_CONTEXT: &str = "deterministic_test_data";
const ROOT_TYPE: &str = "ReceiptsSSZ";
const TARGET_TYPE: &str = "ExecutionAddress";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub branch_order: String,
    pub cases: Vec<ManifestCase>,
    pub dependency_revisions: DependencyRevisions,
    pub format_version: String,
    pub object_kind: String,
    pub producer: Producer,
    pub proof_format: String,
    pub root_context: String,
    pub root_type: String,
    pub schema_digest: String,
    pub schema_id: String,
    pub ssz_semantics: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestCase {
    pub case_id: String,
    pub fixtures: Vec<ManifestFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFixture {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DependencyRevisions {
    pub ethereum_ssz: String,
    pub remerkleable_reference: String,
    pub tree_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Producer {
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofRecord {
    pub format_version: String,
    pub case_id: String,
    pub schema_id: String,
    pub schema_digest: String,
    pub fixture_sha256: String,
    pub object_kind: String,
    pub root_type: String,
    pub path: String,
    pub target_type: String,
    pub value_ssz: String,
    pub receipt_count: usize,
    pub log_counts: Vec<usize>,
    pub gindex: String,
    pub target_node: String,
    pub branch_gindices: Vec<u64>,
    pub proof: Vec<String>,
    pub root: String,
    pub proof_format: String,
    pub root_context: String,
    pub expected_result: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRecord {
    pub format_version: String,
    pub case_id: String,
    pub schema_id: String,
    pub schema_digest: String,
    pub fixture_sha256: String,
    pub object_kind: String,
    pub root_type: String,
    pub receipt_count: usize,
    pub log_counts: Vec<usize>,
    pub root: String,
    pub root_context: String,
}

#[derive(Debug)]
pub struct LoadedFixture {
    pub fixture: ReceiptFixture,
    pub schema_digest: [u8; 32],
    pub fixture_digest: [u8; 32],
    pub log_counts: Vec<usize>,
}

#[derive(Debug)]
pub struct LoadedProofCase {
    pub fixture: LoadedFixture,
    pub proof: ProofRecord,
    pub selected_address: [u8; 20],
    pub target_node: [u8; 32],
    pub gindex: u64,
    pub branch: Vec<[u8; 32]>,
    pub root: [u8; 32],
}

#[derive(Debug)]
pub struct LoadedRootCase {
    pub fixture: LoadedFixture,
    pub record: RootRecord,
    pub root: [u8; 32],
}

pub(crate) fn load_manifest(schema_bytes: &[u8], bytes: &[u8]) -> Result<Manifest, VectorError> {
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    let schema_digest: [u8; 32] = Sha256::digest(schema_bytes).into();
    require(schema_digest == SCHEMA_DIGEST, "schema digest does not match V0")?;
    require(manifest.format_version == FORMAT_VERSION, "unknown manifest format")?;
    require(manifest.schema_id == SCHEMA_ID, "unknown manifest schema")?;
    require(manifest.object_kind == OBJECT_KIND, "unknown object kind")?;
    require(manifest.root_type == ROOT_TYPE, "unknown root type")?;
    require(manifest.proof_format == PROOF_FORMAT, "unknown proof format")?;
    require(manifest.root_context == ROOT_CONTEXT, "unknown root context")?;
    require(manifest.branch_order == "immediate_sibling_first", "unknown branch order")?;
    require(
        decode_fixed::<32>(&manifest.schema_digest)? == schema_digest,
        "manifest schema digest mismatch",
    )?;
    require(!manifest.cases.is_empty(), "manifest has no cases")?;
    require(!manifest.producer.crate_name.is_empty(), "manifest producer is empty")?;
    require(!manifest.producer.version.is_empty(), "manifest producer version is empty")?;
    require(!manifest.ssz_semantics.trim().is_empty(), "SSZ semantics are empty")?;
    require(!manifest.status.trim().is_empty(), "manifest status is empty")?;
    require(
        !manifest.dependency_revisions.ethereum_ssz.is_empty() &&
            !manifest.dependency_revisions.remerkleable_reference.is_empty() &&
            !manifest.dependency_revisions.tree_hash.is_empty(),
        "manifest dependency revision is empty",
    )?;
    Ok(manifest)
}

pub(crate) fn load_fixture(
    schema_bytes: &[u8],
    fixture_bytes: &[u8],
) -> Result<LoadedFixture, VectorError> {
    let schema_digest: [u8; 32] = Sha256::digest(schema_bytes).into();
    require(schema_digest == SCHEMA_DIGEST, "schema digest does not match V0")?;
    let fixture: ReceiptFixture = serde_json::from_slice(fixture_bytes)?;
    validate_receipt_fixture(&fixture)?;
    let fixture_digest: [u8; 32] = Sha256::digest(fixture_bytes).into();
    let log_counts = fixture.receipts.iter().map(|receipt| receipt.logs.len()).collect();
    Ok(LoadedFixture { fixture, schema_digest, fixture_digest, log_counts })
}

pub(crate) fn load_proof_case(
    schema_bytes: &[u8],
    fixture_bytes: &[u8],
    proof_bytes: &[u8],
) -> Result<LoadedProofCase, VectorError> {
    let fixture = load_fixture(schema_bytes, fixture_bytes)?;
    let proof: ProofRecord = serde_json::from_slice(proof_bytes)?;
    validate_common(
        &proof.format_version,
        &proof.schema_id,
        &proof.schema_digest,
        &proof.fixture_sha256,
        &proof.object_kind,
        &proof.root_type,
        proof.receipt_count,
        &proof.log_counts,
        &proof.root_context,
        &fixture,
    )?;
    require(!proof.case_id.trim().is_empty(), "proof case ID is empty")?;
    require(proof.target_type == TARGET_TYPE, "unknown proof target type")?;
    require(proof.proof_format == PROOF_FORMAT, "unknown proof format")?;
    require(proof.expected_result == "verify", "unknown proof result")?;

    let tokens = parse_path(&proof.path)
        .map_err(|error| VectorError::Invalid(format!("invalid vector path: {error:?}")))?;
    let resolved =
        resolve_v0(&tokens).map_err(|_| VectorError::Invalid("unsupported vector path".into()))?;
    let (receipt_index, log_index) = validate_runtime_bounds(resolved, &fixture.log_counts)
        .map_err(|error| {
            VectorError::Invalid(format!("vector index is out of bounds: {error:?}"))
        })?;
    let target = fixture
        .fixture
        .first_target
        .as_ref()
        .ok_or_else(|| VectorError::Invalid("fixture has no proof target".into()))?;
    require(
        target.receipt_index == receipt_index &&
            target.log_index == log_index &&
            target.field == "address",
        "fixture target does not match vector path",
    )?;

    let selected_address =
        decode_fixed::<20>(&fixture.fixture.receipts[receipt_index].logs[log_index].address)?;
    require(
        decode_fixed::<20>(&proof.value_ssz)? == selected_address,
        "proof value does not match selected fixture address",
    )?;
    let target_node = address_target_node(&selected_address)
        .map_err(|_| VectorError::Invalid("selected address has the wrong length".into()))?;
    require(
        decode_fixed::<32>(&proof.target_node)? == target_node,
        "recorded target node mismatch",
    )?;

    let gindex = proof
        .gindex
        .parse::<u64>()
        .map_err(|_| VectorError::Invalid("gindex is not a decimal u64".into()))?;
    let expected_gindex = receipt_log_address_gindex(resolved)
        .map_err(|error| VectorError::Invalid(format!("invalid gindex: {error:?}")))?;
    require(gindex == expected_gindex, "recorded gindex mismatch")?;
    let expected_positions = branch_positions(gindex)
        .map_err(|error| VectorError::Invalid(format!("invalid branch positions: {error:?}")))?;
    require(proof.branch_gindices == expected_positions, "recorded branch gindices mismatch")?;
    let branch =
        proof.proof.iter().map(|node| decode_fixed::<32>(node)).collect::<Result<Vec<_>, _>>()?;
    require(branch.len() == expected_positions.len(), "proof length mismatch")?;
    let root = decode_fixed::<32>(&proof.root)?;

    Ok(LoadedProofCase { fixture, proof, selected_address, target_node, gindex, branch, root })
}

pub(crate) fn load_root_case(
    schema_bytes: &[u8],
    fixture_bytes: &[u8],
    record_bytes: &[u8],
) -> Result<LoadedRootCase, VectorError> {
    let fixture = load_fixture(schema_bytes, fixture_bytes)?;
    let record: RootRecord = serde_json::from_slice(record_bytes)?;
    validate_common(
        &record.format_version,
        &record.schema_id,
        &record.schema_digest,
        &record.fixture_sha256,
        &record.object_kind,
        &record.root_type,
        record.receipt_count,
        &record.log_counts,
        &record.root_context,
        &fixture,
    )?;
    require(!record.case_id.trim().is_empty(), "root case ID is empty")?;
    let root = decode_fixed::<32>(&record.root)?;
    Ok(LoadedRootCase { fixture, record, root })
}

#[allow(clippy::too_many_arguments)]
fn validate_common(
    format_version: &str,
    schema_id: &str,
    schema_digest: &str,
    fixture_sha256: &str,
    object_kind: &str,
    root_type: &str,
    receipt_count: usize,
    log_counts: &[usize],
    root_context: &str,
    fixture: &LoadedFixture,
) -> Result<(), VectorError> {
    require(format_version == FORMAT_VERSION, "unknown vector format")?;
    require(schema_id == SCHEMA_ID, "unknown vector schema")?;
    require(object_kind == OBJECT_KIND, "unknown object kind")?;
    require(root_type == ROOT_TYPE, "unknown root type")?;
    require(root_context == ROOT_CONTEXT, "unknown root context")?;
    require(
        decode_fixed::<32>(schema_digest)? == fixture.schema_digest,
        "recorded schema digest mismatch",
    )?;
    require(
        decode_fixed::<32>(fixture_sha256)? == fixture.fixture_digest,
        "recorded fixture digest mismatch",
    )?;
    require(receipt_count == fixture.fixture.receipts.len(), "recorded receipt count mismatch")?;
    require(log_counts == fixture.log_counts, "recorded log counts mismatch")?;
    Ok(())
}

fn require(condition: bool, message: &str) -> Result<(), VectorError> {
    condition.then_some(()).ok_or_else(|| VectorError::Invalid(message.into()))
}
