use alloy_primitives::B256;
use serde::Deserialize;

use crate::{
    address_target_node, branch_positions, parse_path, receipt_log_address_gindex, resolve_v0,
    validate_runtime_bounds,
    vector::{decode_fixed, validate_receipt_fixture, ReceiptFixture, VectorError},
    SCHEMA_ID,
};

const FORMAT_VERSION: &str = "pureth_receipt_proof_vectors_v0";
const OBJECT_KIND: &str = "receipts";
const PROOF_FORMAT: &str = "merkle_branch_v0";
const ROOT_CONTEXT: &str = "deterministic_test_data";
const ROOT_TYPE: &str = "ReceiptsSSZ";
const TARGET_TYPE: &str = "ExecutionAddress";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProofRecord {
    pub format_version: String,
    pub case_id: String,
    pub schema_id: String,
    #[serde(rename = "schema_digest")]
    _schema_digest: String,
    #[serde(rename = "fixture_sha256")]
    _fixture_sha256: String,
    pub object_kind: String,
    pub root_type: String,
    pub path: String,
    pub target_type: String,
    pub value_ssz: String,
    #[serde(rename = "receipt_count")]
    _receipt_count: usize,
    #[serde(rename = "log_counts")]
    _log_counts: Vec<usize>,
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
pub(crate) struct RootRecord {
    pub format_version: String,
    pub case_id: String,
    pub schema_id: String,
    #[serde(rename = "schema_digest")]
    _schema_digest: String,
    #[serde(rename = "fixture_sha256")]
    _fixture_sha256: String,
    pub object_kind: String,
    pub root_type: String,
    #[serde(rename = "receipt_count")]
    _receipt_count: usize,
    #[serde(rename = "log_counts")]
    _log_counts: Vec<usize>,
    pub root: String,
    pub root_context: String,
}

#[derive(Debug)]
pub(crate) struct LoadedFixture {
    pub fixture: ReceiptFixture,
    pub log_counts: Vec<usize>,
}

#[derive(Debug)]
pub(crate) struct LoadedProofCase {
    pub fixture: LoadedFixture,
    pub proof: ProofRecord,
    pub selected_address: [u8; 20],
    pub gindex: u64,
    pub branch: Vec<B256>,
    pub root: B256,
}

#[derive(Debug)]
pub(crate) struct LoadedRootCase {
    pub fixture: LoadedFixture,
    pub record: RootRecord,
    pub root: B256,
}

pub(crate) fn load_fixture(fixture_bytes: &[u8]) -> Result<LoadedFixture, VectorError> {
    let fixture: ReceiptFixture = serde_json::from_slice(fixture_bytes)?;
    validate_receipt_fixture(&fixture)?;
    let log_counts = fixture.receipts.iter().map(|receipt| receipt.logs.len()).collect();
    Ok(LoadedFixture { fixture, log_counts })
}

pub(crate) fn load_proof_case(
    fixture_bytes: &[u8],
    proof_bytes: &[u8],
) -> Result<LoadedProofCase, VectorError> {
    let fixture = load_fixture(fixture_bytes)?;
    let proof: ProofRecord = serde_json::from_slice(proof_bytes)?;
    validate_common(
        &proof.format_version,
        &proof.schema_id,
        &proof.object_kind,
        &proof.root_type,
        &proof.root_context,
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
        B256::from(decode_fixed::<32>(&proof.target_node)?) == target_node,
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
    let branch = proof
        .proof
        .iter()
        .map(|node| decode_fixed::<32>(node).map(B256::from))
        .collect::<Result<Vec<_>, _>>()?;
    require(branch.len() == expected_positions.len(), "proof length mismatch")?;
    let root = B256::from(decode_fixed::<32>(&proof.root)?);

    Ok(LoadedProofCase { fixture, proof, selected_address, gindex, branch, root })
}

pub(crate) fn load_root_case(
    fixture_bytes: &[u8],
    record_bytes: &[u8],
) -> Result<LoadedRootCase, VectorError> {
    let fixture = load_fixture(fixture_bytes)?;
    let record: RootRecord = serde_json::from_slice(record_bytes)?;
    validate_common(
        &record.format_version,
        &record.schema_id,
        &record.object_kind,
        &record.root_type,
        &record.root_context,
    )?;
    require(!record.case_id.trim().is_empty(), "root case ID is empty")?;
    let root = B256::from(decode_fixed::<32>(&record.root)?);
    Ok(LoadedRootCase { fixture, record, root })
}

fn validate_common(
    format_version: &str,
    schema_id: &str,
    object_kind: &str,
    root_type: &str,
    root_context: &str,
) -> Result<(), VectorError> {
    require(format_version == FORMAT_VERSION, "unknown vector format")?;
    require(schema_id == SCHEMA_ID, "unknown vector schema")?;
    require(object_kind == OBJECT_KIND, "unknown object kind")?;
    require(root_type == ROOT_TYPE, "unknown root type")?;
    require(root_context == ROOT_CONTEXT, "unknown root context")?;
    Ok(())
}

fn require(condition: bool, message: &str) -> Result<(), VectorError> {
    condition.then_some(()).ok_or_else(|| VectorError::Invalid(message.into()))
}
