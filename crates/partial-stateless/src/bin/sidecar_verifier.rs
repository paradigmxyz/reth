//! Standalone verifier + coverage diagnostic for partial-stateless witness sidecars.
//!
//! Two independent checks, neither of which trusts the producer or the transport:
//!
//! 1. CRYPTO INTEGRITY (default) — every missed account proof verifies against
//!    `parent_state_root`, every missed storage proof verifies against its account
//!    `storageRoot` (`AccountProof::verify` covers both), and every supplied
//!    bytecode preimage hashes to a declared missed code hash. This proves the
//!    witness material that IS present is cryptographically anchored to the parent
//!    state root. It does NOT prove the witness is COMPLETE.
//!
//! 2. COVERAGE (`--coverage`) — compares the sidecar against a reference
//!    `debug_executionWitness` (ground truth from a full node) for the same block,
//!    reporting how much of the state actually needed to re-execute the block is
//!    present in the sidecar. This is what catches the BundleState target-source
//!    incompleteness: even with a zero/cold cache (nothing legitimately omitted),
//!    a `bundle_changed_state` source misses most executed bytecode and ancestor
//!    headers, so coverage is well below 100%.
//!
//! Neither mode re-executes the block; full re-execution (`ACCEPT_BLOCK`) is a
//! follow-up. Coverage uses the full node's canonical witness as an oracle, so it
//! is a measurement, not a trustless claim.
//!
//! Caveat (carried from the 2026-06-18 study): a `debug_executionWitness` `keys`
//! list is a flat set of preimages — the account/slot pairing is lost — so the
//! key/preimage coverage number is a proxy. Code coverage (compared by hash) and
//! header coverage are exact.
//!
//! Usage:
//!   sidecar_verifier <path-or-dir> [<path-or-dir> ...]
//!   sidecar_verifier --coverage --witness-dir <dir> <path-or-dir> ...
//!
//! In `--coverage` mode, each sidecar for block N is paired with the reference
//! witness file `<witness-dir>/witness_<N>.json` (the inner `result` object of a
//! `debug_executionWitnessByBlockHash` response: `{state,codes,keys,headers}`).

use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    path::{Path, PathBuf},
    process,
};

use alloy_primitives::{hex, keccak256, Address, B256};
use partial_stateless::{PartialStatelessSidecar, SerializableMultiProof};

type Res<T> = Result<T, Box<dyn Error>>;

/// Reference execution witness (inner `result` of `debug_executionWitness`).
#[derive(serde::Deserialize, Default)]
struct ReferenceWitness {
    // Account/storage trie node preimages — kept for schema completeness; coverage
    // is measured on `keys`/`codes`/`headers`, not raw node count.
    #[serde(default)]
    #[allow(dead_code)]
    state: Vec<String>,
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    keys: Vec<String>,
    #[serde(default)]
    headers: Vec<String>,
}

/// Outcome of crypto-verifying a single sidecar file.
struct CryptoVerdict {
    accounts_checked: usize,
    storage_checked: usize,
    codes_checked: usize,
}

/// Outcome of comparing a sidecar against a reference witness.
struct CoverageReport {
    ref_codes: usize,
    covered_codes: usize,
    ref_keys: usize,
    covered_keys: usize,
    ref_headers: usize,
    covered_headers: usize,
}

impl CoverageReport {
    fn complete(&self) -> bool {
        self.covered_codes == self.ref_codes
            && self.covered_keys == self.ref_keys
            && self.covered_headers == self.ref_headers
    }
}

fn norm_hex(s: &str) -> String {
    s.trim().trim_start_matches("0x").to_ascii_lowercase()
}

/// Crypto-verify the sidecar's proofs and code preimages against `parent_state_root`.
fn crypto_verify(sidecar: &PartialStatelessSidecar) -> Res<CryptoVerdict> {
    let serializable: SerializableMultiProof =
        bincode::deserialize(&sidecar.serialized_multiproof)?;
    let multiproof = serializable.to_multiproof();
    let root = sidecar.parent_state_root;

    // Group missed storage slots by account; missed accounts without storage still
    // get an (empty-slots) entry so their account proof is checked.
    let mut slots_by_account: BTreeMap<Address, Vec<B256>> = BTreeMap::new();
    for addr in &sidecar.raw_targets.missed_accounts {
        slots_by_account.entry(*addr).or_default();
    }
    for (addr, slot) in &sidecar.raw_targets.missed_storage {
        slots_by_account.entry(*addr).or_default().push(*slot);
    }

    let mut storage_checked = 0usize;
    for (addr, slots) in &slots_by_account {
        let account_proof = multiproof
            .account_proof(*addr, slots)
            .map_err(|e| format!("account {addr:?}: could not build proof: {e}"))?;
        account_proof
            .verify(root)
            .map_err(|e| format!("account {addr:?}: proof failed against parent_state_root: {e}"))?;
        storage_checked += slots.len();
    }

    let declared: HashSet<B256> =
        sidecar.raw_targets.missed_code_hashes.iter().copied().collect();
    let mut codes_checked = 0usize;
    for code in &sidecar.missed_bytecodes {
        let h = keccak256(code);
        if !declared.contains(&h) {
            return Err(format!(
                "bytecode preimage keccak {h:?} ({} bytes) not in declared missed_code_hashes",
                code.len()
            )
            .into());
        }
        codes_checked += 1;
    }

    Ok(CryptoVerdict {
        accounts_checked: slots_by_account.len(),
        storage_checked,
        codes_checked,
    })
}

/// Compare a sidecar's coverage against a reference execution witness.
fn coverage(sidecar: &PartialStatelessSidecar, witness: &ReferenceWitness) -> CoverageReport {
    // Codes: compare by hash (exact). Reference `codes` are bytecodes; hash each.
    let ref_code_hashes: HashSet<String> = witness
        .codes
        .iter()
        .filter_map(|c| hex::decode(c.trim_start_matches("0x")).ok())
        .map(|bytes| norm_hex(&keccak256(&bytes).to_string()))
        .collect();
    let sidecar_code_hashes: HashSet<String> = sidecar
        .raw_targets
        .missed_code_hashes
        .iter()
        .map(|h| norm_hex(&h.to_string()))
        .collect();
    let covered_codes = ref_code_hashes.intersection(&sidecar_code_hashes).count();

    // Keys: proxy (flat preimage list; account/slot pairing lost upstream).
    let ref_keys: HashSet<String> = witness.keys.iter().map(|k| norm_hex(k)).collect();
    let mut sidecar_keys: HashSet<String> = HashSet::new();
    for addr in &sidecar.raw_targets.missed_accounts {
        sidecar_keys.insert(norm_hex(&hex::encode(addr)));
    }
    for (_addr, slot) in &sidecar.raw_targets.missed_storage {
        sidecar_keys.insert(norm_hex(&hex::encode(slot)));
    }
    let covered_keys = ref_keys.intersection(&sidecar_keys).count();

    // Headers: the current sidecar schema carries none.
    let ref_headers = witness.headers.len();

    CoverageReport {
        ref_codes: ref_code_hashes.len(),
        covered_codes,
        ref_keys: ref_keys.len(),
        covered_keys,
        ref_headers,
        covered_headers: 0,
    }
}

fn pct(num: usize, den: usize) -> String {
    if den == 0 {
        "n/a".to_string()
    } else {
        format!("{:.1}%", 100.0 * num as f64 / den as f64)
    }
}

fn process_sidecar(path: &Path, witness_dir: Option<&Path>) -> Res<bool> {
    let bytes = std::fs::read(path)?;
    let sidecar: PartialStatelessSidecar = bincode::deserialize(&bytes)?;
    let block = sidecar.block_number;

    // 1. Crypto integrity.
    let v = crypto_verify(&sidecar)?;
    println!(
        "PROOF_OK   block={block} accounts={} storage={} codes={} root={:?}",
        v.accounts_checked, v.storage_checked, v.codes_checked, sidecar.parent_state_root,
    );

    // 2. Coverage (optional).
    if let Some(dir) = witness_dir {
        let wpath = dir.join(format!("witness_{block}.json"));
        let witness: ReferenceWitness = serde_json::from_slice(&std::fs::read(&wpath)?)?;
        let c = coverage(&sidecar, &witness);
        println!(
            "  COVERAGE block={block} codes={}/{} ({}) keys={}/{} ({}, proxy) headers={}/{} :: {}",
            c.covered_codes,
            c.ref_codes,
            pct(c.covered_codes, c.ref_codes),
            c.covered_keys,
            c.ref_keys,
            pct(c.covered_keys, c.ref_keys),
            c.covered_headers,
            c.ref_headers,
            if c.complete() { "COMPLETE_FOR_REEXECUTION" } else { "INCOMPLETE_FOR_REEXECUTION" },
        );
    }

    Ok(true)
}

/// Expand a path argument into the list of `.bin` sidecar files it refers to.
fn collect_sidecar_files(arg: &str) -> Res<Vec<PathBuf>> {
    let path = PathBuf::from(arg);
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&path)?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bin"))
            .collect();
        files.sort();
        Ok(files)
    } else if path.is_file() {
        Ok(vec![path])
    } else {
        Err(format!("path does not exist: {arg}").into())
    }
}

fn main() {
    let mut coverage_mode = false;
    let mut witness_dir: Option<PathBuf> = None;
    let mut inputs: Vec<String> = Vec::new();

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--coverage" => coverage_mode = true,
            "--witness-dir" => match it.next() {
                Some(d) => witness_dir = Some(PathBuf::from(d)),
                None => {
                    eprintln!("--witness-dir requires a path");
                    process::exit(2);
                }
            },
            _ => inputs.push(arg),
        }
    }

    if inputs.is_empty() {
        eprintln!("usage: sidecar_verifier [--coverage --witness-dir <dir>] <path-or-dir> ...");
        process::exit(2);
    }
    if coverage_mode && witness_dir.is_none() {
        eprintln!("--coverage requires --witness-dir <dir>");
        process::exit(2);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    for arg in &inputs {
        match collect_sidecar_files(arg) {
            Ok(mut found) => files.append(&mut found),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(2);
            }
        }
    }
    if files.is_empty() {
        eprintln!("no .bin sidecar files found in arguments");
        process::exit(2);
    }

    let wdir = witness_dir.as_deref().filter(|_| coverage_mode);
    let mut passed = 0usize;
    let mut failed = 0usize;
    for file in &files {
        match process_sidecar(file, wdir) {
            Ok(_) => passed += 1,
            Err(e) => {
                failed += 1;
                println!("PROOF_FAIL file={} :: {e}", file.display());
            }
        }
    }

    println!("---");
    println!("verified {} sidecar(s): {passed} PROOF_OK, {failed} PROOF_FAIL", files.len());
    if failed > 0 {
        process::exit(1);
    }
}
