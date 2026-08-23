use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::{
    vector::{decode_fixed, VectorError},
    vector_records::{
        load_manifest, load_proof_case, load_root_case, LoadedProofCase, LoadedRootCase, Manifest,
        ManifestCase, ManifestFixture,
    },
};

pub fn load_manifest_proof_case(
    schema_bytes: &[u8],
    manifest_bytes: &[u8],
    case_id: &str,
    fixture_path: &str,
    fixture_bytes: &[u8],
    proof_bytes: &[u8],
) -> Result<LoadedProofCase, VectorError> {
    let manifest = load_manifest(schema_bytes, manifest_bytes)?;
    let fixture_digest = bind_fixture(&manifest, case_id, fixture_path, fixture_bytes)?;
    let loaded = load_proof_case(schema_bytes, fixture_bytes, proof_bytes)?;
    require(
        loaded.proof.case_id == case_id,
        "proof case ID does not match the selected manifest case",
    )?;
    require(
        loaded.fixture.fixture_digest == fixture_digest,
        "loaded proof fixture does not match the selected manifest fixture",
    )?;
    Ok(loaded)
}

pub fn load_manifest_root_case(
    schema_bytes: &[u8],
    manifest_bytes: &[u8],
    case_id: &str,
    fixture_path: &str,
    fixture_bytes: &[u8],
    record_bytes: &[u8],
) -> Result<LoadedRootCase, VectorError> {
    let manifest = load_manifest(schema_bytes, manifest_bytes)?;
    let fixture_digest = bind_fixture(&manifest, case_id, fixture_path, fixture_bytes)?;
    let loaded = load_root_case(schema_bytes, fixture_bytes, record_bytes)?;
    require(
        loaded.record.case_id == case_id,
        "root case ID does not match the selected manifest case",
    )?;
    require(
        loaded.fixture.fixture_digest == fixture_digest,
        "loaded root fixture does not match the selected manifest fixture",
    )?;
    Ok(loaded)
}

fn bind_fixture(
    manifest: &Manifest,
    case_id: &str,
    fixture_path: &str,
    fixture_bytes: &[u8],
) -> Result<[u8; 32], VectorError> {
    validate_manifest_paths(manifest)?;
    let case = select_case(manifest, case_id)?;
    let fixture = select_fixture(case, fixture_path)?;
    let digest: [u8; 32] = Sha256::digest(fixture_bytes).into();
    require(
        decode_fixed::<32>(&fixture.sha256)? == digest,
        "fixture bytes do not match the selected manifest hash",
    )?;
    Ok(digest)
}

fn select_case<'a>(manifest: &'a Manifest, case_id: &str) -> Result<&'a ManifestCase, VectorError> {
    manifest
        .cases
        .iter()
        .find(|case| case.case_id == case_id)
        .ok_or_else(|| VectorError::Invalid("requested manifest case does not exist".into()))
}

fn select_fixture<'a>(
    case: &'a ManifestCase,
    fixture_path: &str,
) -> Result<&'a ManifestFixture, VectorError> {
    case.fixtures.iter().find(|fixture| fixture.path == fixture_path).ok_or_else(|| {
        VectorError::Invalid("fixture path is not part of the selected manifest case".into())
    })
}

fn validate_manifest_paths(manifest: &Manifest) -> Result<(), VectorError> {
    let mut case_ids = HashSet::new();
    for case in &manifest.cases {
        require(!case.case_id.trim().is_empty(), "manifest case ID is empty")?;
        require(case_ids.insert(case.case_id.as_str()), "manifest case ID is duplicated")?;
        require(!case.fixtures.is_empty(), "manifest case has no fixtures")?;
        let mut fixture_paths = HashSet::new();
        for fixture in &case.fixtures {
            require(
                safe_relative_path(&fixture.path),
                "manifest fixture path must stay inside the vector root",
            )?;
            require(
                fixture_paths.insert(fixture.path.as_str()),
                "manifest fixture path is duplicated within its case",
            )?;
        }
    }
    Ok(())
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty() &&
        !path.starts_with('/') &&
        !path.contains(['\\', ':']) &&
        path.split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn require(condition: bool, message: &str) -> Result<(), VectorError> {
    condition.then_some(()).ok_or_else(|| VectorError::Invalid(message.into()))
}
