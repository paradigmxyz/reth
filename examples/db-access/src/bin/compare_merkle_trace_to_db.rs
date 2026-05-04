#![warn(unused_crate_dependencies)]

use alloy_primitives::{B256, U256};
use clap::{Parser, ValueEnum};
use eyre::{bail, eyre, Context, Result};
use reth_chainspec::{ChainSpec, DEV, HOLESKY, HOODI, MAINNET, SEPOLIA};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_ethereum::{node::EthereumNode, provider::providers::ReadOnlyConfig, tasks::Runtime};
use reth_storage_api::{DBProvider, StorageSettingsCache};
use reth_trie::{
    trie_cursor::{TrieCursor, TrieCursorFactory},
    Nibbles, StorageRoot,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseStorageRoot, DatabaseTrieCursorFactory, TrieTableAdapter,
};
use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ChainArg {
    Mainnet,
    Sepolia,
    Holesky,
    Hoodi,
    Dev,
}

impl ChainArg {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Sepolia => "sepolia",
            Self::Holesky => "holesky",
            Self::Hoodi => "hoodi",
            Self::Dev => "dev",
        }
    }

    fn chain_spec(self) -> Arc<ChainSpec> {
        match self {
            Self::Mainnet => MAINNET.clone(),
            Self::Sepolia => SEPOLIA.clone(),
            Self::Holesky => HOLESKY.clone(),
            Self::Hoodi => HOODI.clone(),
            Self::Dev => DEV.clone(),
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    about = "Compare a failed Merkle unwind trace against DB-backed trie/account/storage state."
)]
struct Args {
    /// Path to the failed restart trace log.
    trace_file: PathBuf,

    /// Reth datadir to treat as DB ground truth.
    datadir: PathBuf,

    /// Chain spec used to open the datadir.
    #[arg(long, value_enum, default_value_t = ChainArg::Hoodi)]
    chain: ChainArg,

    /// Cap the number of reported results per section.
    #[arg(long)]
    max_results: Option<usize>,
}

#[derive(Debug, Clone)]
struct ObservedStateBranch {
    path: Nibbles,
    observed_hash: B256,
    children_are_in_trie: bool,
    first_line: usize,
    occurrences: usize,
}

#[derive(Debug, Clone)]
struct ObservedAccountLeaf {
    hashed_address: B256,
    nonce: u64,
    balance: U256,
    bytecode_hash: Option<B256>,
    first_line: usize,
    occurrences: usize,
}

#[derive(Debug, Clone)]
struct ObservedStorageBranch {
    hashed_address: B256,
    path: Nibbles,
    observed_hash: B256,
    children_are_in_trie: bool,
    first_line: usize,
    occurrences: usize,
}

#[derive(Debug, Clone)]
struct ObservedStorageLeaf {
    hashed_address: B256,
    hashed_slot: B256,
    value: U256,
    first_line: usize,
    occurrences: usize,
}

#[derive(Debug, Clone)]
struct ObservedStorageRoot {
    hashed_address: B256,
    root: B256,
    first_line: usize,
    occurrences: usize,
}

#[derive(Debug, Default)]
struct TraceData {
    state_branches: Vec<ObservedStateBranch>,
    account_leaves: Vec<ObservedAccountLeaf>,
    storage_branches: Vec<ObservedStorageBranch>,
    storage_leaves: Vec<ObservedStorageLeaf>,
    storage_roots: Vec<ObservedStorageRoot>,
}

#[derive(Debug, Clone)]
struct BranchCandidate {
    location: String,
    expected_hash: Option<B256>,
    children_are_in_trie: bool,
}

impl BranchCandidate {
    fn detail(&self) -> String {
        format!(
            "db_candidate={} expected_hash={} expected_children_are_in_trie={}",
            self.location,
            option_b256(self.expected_hash),
            self.children_are_in_trie
        )
    }
}

#[derive(Debug, Default)]
struct BranchLookup {
    candidates: Vec<BranchCandidate>,
    notes: Vec<String>,
}

impl BranchLookup {
    fn matches(&self, observed_hash: B256, children_are_in_trie: bool) -> bool {
        self.candidates.iter().any(|candidate| {
            candidate.expected_hash == Some(observed_hash) &&
                candidate.children_are_in_trie == children_are_in_trie
        })
    }

    fn details(&self) -> Vec<String> {
        let mut details = Vec::new();
        if self.candidates.is_empty() {
            details.push("db_candidates=none".to_string());
        } else {
            details.extend(self.candidates.iter().map(BranchCandidate::detail));
        }
        details.extend(self.notes.iter().cloned());
        details
    }
}

#[derive(Debug, Clone)]
struct Mismatch {
    context: MismatchContext,
    path: Nibbles,
    first_line: usize,
    kind_rank: u8,
    headline: String,
    details: Vec<String>,
    suppressed_descendants: usize,
}

#[derive(Debug, Clone)]
struct Diagnostic {
    context: MismatchContext,
    first_line: usize,
    headline: String,
    details: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MismatchContext {
    State,
    Storage(B256),
}

impl std::fmt::Display for MismatchContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State => f.write_str("state"),
            Self::Storage(hashed_address) => write!(f, "storage:{hashed_address}"),
        }
    }
}

#[derive(Debug, Default)]
struct ComparisonResults {
    direct_mismatches: Vec<Mismatch>,
    diagnostics: Vec<Diagnostic>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !args.trace_file.is_file() {
        bail!("trace file does not exist: {}", args.trace_file.display());
    }
    if !args.datadir.is_dir() {
        bail!("datadir does not exist: {}", args.datadir.display());
    }

    let trace = parse_trace(&args.trace_file)?;

    let runtime = Runtime::test();
    let factory = EthereumNode::provider_factory_builder().open_read_only(
        args.chain.chain_spec(),
        ReadOnlyConfig::from_datadir(args.datadir.clone()),
        runtime,
    )?;
    let provider = factory.provider()?;

    let results =
        reth_trie_db::with_adapter!(provider, |A| compare_with_adapter::<_, A>(&provider, &trace))?;

    let total_direct_mismatches = results.direct_mismatches.len();
    let mut outermost = retain_outermost_mismatches(results.direct_mismatches);
    let suppressed_direct_descendants =
        outermost.iter().map(|mismatch| mismatch.suppressed_descendants).sum::<usize>();
    let diagnostic_count = results.diagnostics.len();
    let max_results = args.max_results.unwrap_or(usize::MAX);

    println!("trace_file={}", args.trace_file.display());
    println!("datadir={}", args.datadir.display());
    println!("chain={}", args.chain.as_str());
    println!(
        "trace_observations state_branches={} account_leaves={} storage_branches={} storage_leaves={} storage_roots={}",
        trace.state_branches.len(),
        trace.account_leaves.len(),
        trace.storage_branches.len(),
        trace.storage_leaves.len(),
        trace.storage_roots.len(),
    );
    println!(
        "direct_mismatches total={} outermost={} suppressed_descendants={}",
        total_direct_mismatches,
        outermost.len(),
        suppressed_direct_descendants,
    );
    println!("reported_direct_mismatches={}", outermost.len().min(max_results));
    println!("storage_root_diagnostics={diagnostic_count}");
    println!("reported_storage_root_diagnostics={}", diagnostic_count.min(max_results));

    for mismatch in outermost.drain(..).take(max_results) {
        println!();
        println!("[{}] {}", mismatch.context, mismatch.headline);
        for detail in mismatch.details {
            println!("  {detail}");
        }
        if mismatch.suppressed_descendants > 0 {
            println!("  suppressed_descendants={}", mismatch.suppressed_descendants);
        }
    }

    for diagnostic in results.diagnostics.into_iter().take(max_results) {
        println!();
        println!("[{}] {}", diagnostic.context, diagnostic.headline);
        for detail in diagnostic.details {
            println!("  {detail}");
        }
    }

    Ok(())
}

fn compare_with_adapter<P, A>(provider: &P, trace: &TraceData) -> Result<ComparisonResults>
where
    P: DBProvider,
    A: TrieTableAdapter,
{
    let tx = provider.tx_ref();
    let trie_factory = DatabaseTrieCursorFactory::<_, A>::new(tx);
    let mut state_trie_cursor = trie_factory.account_trie_cursor()?;
    let mut hashed_accounts_cursor = tx.cursor_read::<tables::HashedAccounts>()?;
    let mut hashed_storage_cursor = tx.cursor_dup_read::<tables::HashedStorages>()?;

    let mut results = ComparisonResults::default();

    for observed in &trace.state_branches {
        let lookup = lookup_branch(&mut state_trie_cursor, &observed.path)?;
        if lookup.matches(observed.observed_hash, observed.children_are_in_trie) {
            continue;
        }

        let mut details = vec![
            format!("observed_hash={}", observed.observed_hash),
            format!("observed_children_are_in_trie={}", observed.children_are_in_trie),
            format!("trace_occurrences={}", observed.occurrences),
        ];
        details.extend(lookup.details());

        results.direct_mismatches.push(Mismatch {
            context: MismatchContext::State,
            path: observed.path,
            first_line: observed.first_line,
            kind_rank: 0,
            headline: format!(
                "branch mismatch path={} line={}",
                nibbles_hex(&observed.path),
                observed.first_line
            ),
            details,
            suppressed_descendants: 0,
        });
    }

    for observed in &trace.account_leaves {
        match hashed_accounts_cursor.seek_exact(observed.hashed_address)? {
            Some((_, account)) => {
                let mut diffs = Vec::new();
                if observed.nonce != account.nonce {
                    diffs.push(format!(
                        "nonce observed={} expected={}",
                        observed.nonce, account.nonce
                    ));
                }
                if observed.balance != account.balance {
                    diffs.push(format!(
                        "balance observed={} expected={}",
                        observed.balance, account.balance
                    ));
                }
                if observed.bytecode_hash != account.bytecode_hash {
                    diffs.push(format!(
                        "bytecode_hash observed={} expected={}",
                        option_b256(observed.bytecode_hash),
                        option_b256(account.bytecode_hash)
                    ));
                }
                if diffs.is_empty() {
                    continue;
                }

                diffs.push(format!("trace_occurrences={}", observed.occurrences));
                results.direct_mismatches.push(Mismatch {
                    context: MismatchContext::State,
                    path: Nibbles::unpack(observed.hashed_address),
                    first_line: observed.first_line,
                    kind_rank: 1,
                    headline: format!(
                        "account leaf mismatch hashed_address={} line={}",
                        observed.hashed_address, observed.first_line
                    ),
                    details: diffs,
                    suppressed_descendants: 0,
                });
            }
            None => results.direct_mismatches.push(Mismatch {
                context: MismatchContext::State,
                path: Nibbles::unpack(observed.hashed_address),
                first_line: observed.first_line,
                kind_rank: 1,
                headline: format!(
                    "account leaf missing from DB hashed state hashed_address={} line={}",
                    observed.hashed_address, observed.first_line
                ),
                details: vec![
                    format!("nonce={}", observed.nonce),
                    format!("balance={}", observed.balance),
                    format!("bytecode_hash={}", option_b256(observed.bytecode_hash)),
                    format!("trace_occurrences={}", observed.occurrences),
                ],
                suppressed_descendants: 0,
            }),
        }
    }

    let mut storage_branches_by_address = BTreeMap::<B256, Vec<&ObservedStorageBranch>>::new();
    for observed in &trace.storage_branches {
        storage_branches_by_address.entry(observed.hashed_address).or_default().push(observed);
    }

    for (hashed_address, branches) in storage_branches_by_address {
        let mut storage_trie_cursor = trie_factory.storage_trie_cursor(hashed_address)?;
        for observed in branches {
            let lookup = lookup_branch(&mut storage_trie_cursor, &observed.path)?;
            if lookup.matches(observed.observed_hash, observed.children_are_in_trie) {
                continue;
            }

            let mut details = vec![
                format!("observed_hash={}", observed.observed_hash),
                format!("observed_children_are_in_trie={}", observed.children_are_in_trie),
                format!("trace_occurrences={}", observed.occurrences),
            ];
            details.extend(lookup.details());

            results.direct_mismatches.push(Mismatch {
                context: MismatchContext::Storage(observed.hashed_address),
                path: observed.path,
                first_line: observed.first_line,
                kind_rank: 0,
                headline: format!(
                    "branch mismatch hashed_address={} path={} line={}",
                    observed.hashed_address,
                    nibbles_hex(&observed.path),
                    observed.first_line
                ),
                details,
                suppressed_descendants: 0,
            });
        }
    }

    for observed in &trace.storage_leaves {
        match hashed_storage_cursor.seek_by_key_subkey(observed.hashed_address, observed.hashed_slot)? {
            Some(entry) if entry.key == observed.hashed_slot => {
                if observed.value == entry.value {
                    continue;
                }

                results.direct_mismatches.push(Mismatch {
                    context: MismatchContext::Storage(observed.hashed_address),
                    path: Nibbles::unpack(observed.hashed_slot),
                    first_line: observed.first_line,
                    kind_rank: 1,
                    headline: format!(
                        "storage leaf mismatch hashed_address={} hashed_slot={} line={}",
                        observed.hashed_address, observed.hashed_slot, observed.first_line
                    ),
                    details: vec![
                        format!("observed_value={}", observed.value),
                        format!("expected_value={}", entry.value),
                        format!("trace_occurrences={}", observed.occurrences),
                    ],
                    suppressed_descendants: 0,
                });
            }
            _ => results.direct_mismatches.push(Mismatch {
                context: MismatchContext::Storage(observed.hashed_address),
                path: Nibbles::unpack(observed.hashed_slot),
                first_line: observed.first_line,
                kind_rank: 1,
                headline: format!(
                    "storage leaf missing from DB hashed state hashed_address={} hashed_slot={} line={}",
                    observed.hashed_address, observed.hashed_slot, observed.first_line
                ),
                details: vec![
                    format!("observed_value={}", observed.value),
                    format!("trace_occurrences={}", observed.occurrences),
                ],
                suppressed_descendants: 0,
            }),
        }
    }

    let mut storage_root_cache = HashMap::<B256, B256>::new();
    for observed in &trace.storage_roots {
        let expected_root = match storage_root_cache.get(&observed.hashed_address) {
            Some(root) => *root,
            None => {
                let root = storage_root_for_hashed_address::<_, A>(tx, observed.hashed_address)?;
                storage_root_cache.insert(observed.hashed_address, root);
                root
            }
        };

        if observed.root == expected_root {
            continue;
        }

        results.diagnostics.push(Diagnostic {
            context: MismatchContext::Storage(observed.hashed_address),
            first_line: observed.first_line,
            headline: format!(
                "storage root mismatch hashed_address={} line={}",
                observed.hashed_address, observed.first_line
            ),
            details: vec![
                format!("observed_root={}", observed.root),
                format!("expected_root={expected_root}"),
                format!("trace_occurrences={}", observed.occurrences),
            ],
        });
    }

    results.diagnostics.sort_by(|left, right| {
        left.first_line.cmp(&right.first_line).then(left.context.cmp(&right.context))
    });

    Ok(results)
}

fn storage_root_for_hashed_address<TX, A>(tx: &TX, hashed_address: B256) -> Result<B256>
where
    TX: DbTx,
    A: TrieTableAdapter,
{
    <DbStorageRoot<'_, TX, A> as DatabaseStorageRoot<_>>::from_tx_hashed(tx, hashed_address)
        .root()
        .with_context(|| format!("compute storage root for hashed address {hashed_address}"))
}

fn lookup_branch<C>(
    cursor: &mut C,
    path: &Nibbles,
) -> Result<BranchLookup, reth_db_api::DatabaseError>
where
    C: TrieCursor,
{
    let mut lookup = BranchLookup::default();

    if let Some((_, node)) = cursor.seek_exact(*path)? {
        if let Some(root_hash) = node.root_hash {
            lookup.candidates.push(BranchCandidate {
                location: format!("parent_root path={}", nibbles_hex(path)),
                expected_hash: Some(root_hash),
                children_are_in_trie: true,
            });
        } else {
            lookup.notes.push(format!(
                "exact_branch_node_present path={} root_hash=None",
                nibbles_hex(path)
            ));
        }
    }

    if !path.is_empty() {
        let parent_path = path.slice(..path.len() - 1);
        let nibble = path.get_unchecked(path.len() - 1);
        match cursor.seek_exact(parent_path)? {
            Some((_, node)) => {
                if node.state_mask.is_bit_set(nibble) {
                    lookup.candidates.push(BranchCandidate {
                        location: format!(
                            "child parent_path={} nibble={}",
                            nibbles_hex(&parent_path),
                            nibble_hex(nibble)
                        ),
                        expected_hash: node
                            .hash_mask
                            .is_bit_set(nibble)
                            .then(|| node.hash_for_nibble(nibble)),
                        children_are_in_trie: node.tree_mask.is_bit_set(nibble),
                    });
                } else {
                    lookup.notes.push(format!(
                        "parent_branch_present path={} missing_state_nibble={}",
                        nibbles_hex(&parent_path),
                        nibble_hex(nibble)
                    ));
                }
            }
            None => lookup
                .notes
                .push(format!("parent_branch_missing path={}", nibbles_hex(&parent_path))),
        }
    }

    Ok(lookup)
}

fn retain_outermost_mismatches(mut mismatches: Vec<Mismatch>) -> Vec<Mismatch> {
    mismatches.sort_by(|a, b| {
        a.context
            .cmp(&b.context)
            .then(a.path.len().cmp(&b.path.len()))
            .then(a.kind_rank.cmp(&b.kind_rank))
            .then(a.first_line.cmp(&b.first_line))
    });

    let mut kept = Vec::<Mismatch>::new();
    'outer: for mismatch in mismatches {
        for existing in &mut kept {
            if existing.context == mismatch.context && mismatch.path.starts_with(&existing.path) {
                existing.suppressed_descendants += 1;
                continue 'outer;
            }
        }
        kept.push(mismatch);
    }

    kept.sort_by(|a, b| {
        a.path
            .len()
            .cmp(&b.path.len())
            .then(a.kind_rank.cmp(&b.kind_rank))
            .then(a.first_line.cmp(&b.first_line))
    });
    kept
}

fn parse_trace(path: &Path) -> Result<TraceData> {
    let file = File::open(path).with_context(|| format!("open trace file {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut state_branches = HashMap::<(Nibbles, B256, bool), ObservedStateBranch>::new();
    let mut account_leaves = HashMap::<(B256, u64, U256, Option<B256>), ObservedAccountLeaf>::new();
    let mut storage_branches = HashMap::<(B256, Nibbles, B256, bool), ObservedStorageBranch>::new();
    let mut storage_leaves = HashMap::<(B256, B256, U256), ObservedStorageLeaf>::new();
    let mut storage_roots = HashMap::<(B256, B256), ObservedStorageRoot>::new();

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line?;

        if line.contains("trie::storage_root: calculated storage root") {
            let root = parse_b256(
                extract_after(&line, "calculated storage root root=")?.split(' ').next().unwrap(),
            )?;
            let hashed_address =
                parse_b256(extract_after(&line, "hashed_address=")?.split(' ').next().unwrap())?;
            upsert_storage_root(&mut storage_roots, hashed_address, root, line_number);
            continue;
        }

        if !line.contains("trie::node_iter: return=Ok(Some(") {
            continue;
        }

        if line.contains("trie_type=State") && line.contains("Branch(TrieBranchNode {") {
            let path = parse_trace_nibbles(extract_between(&line, "key: Nibbles(", "), value:")?)?;
            let observed_hash =
                parse_b256(extract_between(&line, "value: ", ", children_are_in_trie:")?)?;
            let children_are_in_trie =
                parse_bool(extract_between(&line, "children_are_in_trie: ", " })))")?)?;
            upsert_state_branch(
                &mut state_branches,
                path,
                observed_hash,
                children_are_in_trie,
                line_number,
            );
            continue;
        }

        if line.contains("trie_type=Storage") && line.contains("Branch(TrieBranchNode {") {
            let hashed_address = parse_b256(extract_storage_address(&line)?)?;
            let path = parse_trace_nibbles(extract_between(&line, "key: Nibbles(", "), value:")?)?;
            let observed_hash =
                parse_b256(extract_between(&line, "value: ", ", children_are_in_trie:")?)?;
            let children_are_in_trie =
                parse_bool(extract_between(&line, "children_are_in_trie: ", " })))")?)?;
            upsert_storage_branch(
                &mut storage_branches,
                hashed_address,
                path,
                observed_hash,
                children_are_in_trie,
                line_number,
            );
            continue;
        }

        if line.contains("trie_type=State") && line.contains("Leaf(") && line.contains("Account {")
        {
            let hashed_address = parse_b256(extract_between(&line, "Leaf(", ", Account {")?)?;
            let nonce = extract_between(&line, "nonce: ", ", balance:")?
                .parse::<u64>()
                .with_context(|| format!("parse nonce on line {line_number}"))?;
            let balance = extract_between(&line, "balance: ", ", bytecode_hash:")?
                .parse::<U256>()
                .with_context(|| format!("parse balance on line {line_number}"))?;
            let bytecode_hash =
                parse_optional_b256(extract_between(&line, "bytecode_hash: ", " })))")?)?;
            upsert_account_leaf(
                &mut account_leaves,
                hashed_address,
                nonce,
                balance,
                bytecode_hash,
                line_number,
            );
            continue;
        }

        if line.contains("trie_type=Storage") && line.contains("Leaf(") {
            let hashed_address = parse_b256(extract_storage_address(&line)?)?;
            let leaf = extract_after(&line, "Leaf(")?;
            let (hashed_slot, value) = leaf
                .split_once(", ")
                .ok_or_else(|| eyre!("invalid storage leaf on line {line_number}"))?;
            let hashed_slot = parse_b256(hashed_slot)?;
            let value = value
                .split_once(")))")
                .map(|(value, _)| value)
                .ok_or_else(|| eyre!("invalid storage leaf terminator on line {line_number}"))?
                .parse::<U256>()
                .with_context(|| format!("parse storage value on line {line_number}"))?;
            upsert_storage_leaf(
                &mut storage_leaves,
                hashed_address,
                hashed_slot,
                value,
                line_number,
            );
        }
    }

    Ok(TraceData {
        state_branches: into_sorted(state_branches),
        account_leaves: into_sorted(account_leaves),
        storage_branches: into_sorted(storage_branches),
        storage_leaves: into_sorted(storage_leaves),
        storage_roots: into_sorted(storage_roots),
    })
}

fn upsert_state_branch(
    state_branches: &mut HashMap<(Nibbles, B256, bool), ObservedStateBranch>,
    path: Nibbles,
    observed_hash: B256,
    children_are_in_trie: bool,
    line_number: usize,
) {
    state_branches
        .entry((path, observed_hash, children_are_in_trie))
        .and_modify(|entry| entry.occurrences += 1)
        .or_insert(ObservedStateBranch {
            path,
            observed_hash,
            children_are_in_trie,
            first_line: line_number,
            occurrences: 1,
        });
}

fn upsert_account_leaf(
    account_leaves: &mut HashMap<(B256, u64, U256, Option<B256>), ObservedAccountLeaf>,
    hashed_address: B256,
    nonce: u64,
    balance: U256,
    bytecode_hash: Option<B256>,
    line_number: usize,
) {
    account_leaves
        .entry((hashed_address, nonce, balance, bytecode_hash))
        .and_modify(|entry| entry.occurrences += 1)
        .or_insert(ObservedAccountLeaf {
            hashed_address,
            nonce,
            balance,
            bytecode_hash,
            first_line: line_number,
            occurrences: 1,
        });
}

fn upsert_storage_branch(
    storage_branches: &mut HashMap<(B256, Nibbles, B256, bool), ObservedStorageBranch>,
    hashed_address: B256,
    path: Nibbles,
    observed_hash: B256,
    children_are_in_trie: bool,
    line_number: usize,
) {
    storage_branches
        .entry((hashed_address, path, observed_hash, children_are_in_trie))
        .and_modify(|entry| entry.occurrences += 1)
        .or_insert(ObservedStorageBranch {
            hashed_address,
            path,
            observed_hash,
            children_are_in_trie,
            first_line: line_number,
            occurrences: 1,
        });
}

fn upsert_storage_leaf(
    storage_leaves: &mut HashMap<(B256, B256, U256), ObservedStorageLeaf>,
    hashed_address: B256,
    hashed_slot: B256,
    value: U256,
    line_number: usize,
) {
    storage_leaves
        .entry((hashed_address, hashed_slot, value))
        .and_modify(|entry| entry.occurrences += 1)
        .or_insert(ObservedStorageLeaf {
            hashed_address,
            hashed_slot,
            value,
            first_line: line_number,
            occurrences: 1,
        });
}

fn upsert_storage_root(
    storage_roots: &mut HashMap<(B256, B256), ObservedStorageRoot>,
    hashed_address: B256,
    root: B256,
    line_number: usize,
) {
    storage_roots
        .entry((hashed_address, root))
        .and_modify(|entry| entry.occurrences += 1)
        .or_insert(ObservedStorageRoot {
            hashed_address,
            root,
            first_line: line_number,
            occurrences: 1,
        });
}

fn into_sorted<K, T>(bucket: HashMap<K, T>) -> Vec<T>
where
    K: std::hash::Hash + Eq,
    T: TraceLine,
{
    let mut values = bucket.into_values().collect::<Vec<_>>();
    values.sort_by_key(|left| left.first_line());
    values
}

fn parse_b256(value: &str) -> Result<B256> {
    value.parse::<B256>().with_context(|| format!("parse B256 from {value}"))
}

fn parse_optional_b256(value: &str) -> Result<Option<B256>> {
    if value == "None" {
        return Ok(None)
    }
    let inner = value
        .strip_prefix("Some(")
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| eyre!("invalid optional B256: {value}"))?;
    Ok(Some(parse_b256(inner)?))
}

fn parse_trace_nibbles(value: &str) -> Result<Nibbles> {
    let hex = value
        .strip_prefix("0x")
        .ok_or_else(|| eyre!("expected nibble string with 0x prefix: {value}"))?;
    let mut nibbles = Vec::with_capacity(hex.len());
    for ch in hex.bytes() {
        let nibble = match ch {
            b'0'..=b'9' => ch - b'0',
            b'a'..=b'f' => ch - b'a' + 10,
            b'A'..=b'F' => ch - b'A' + 10,
            _ => bail!("invalid hex nibble in {value}"),
        };
        nibbles.push(nibble);
    }
    Ok(Nibbles::from_nibbles_unchecked(nibbles))
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("invalid boolean value: {value}"),
    }
}

fn extract_after<'a>(line: &'a str, prefix: &str) -> Result<&'a str> {
    line.split_once(prefix).map(|(_, rest)| rest).ok_or_else(|| eyre!("missing {prefix:?} in line"))
}

fn extract_between<'a>(line: &'a str, start: &str, end: &str) -> Result<&'a str> {
    let rest = extract_after(line, start)?;
    rest.split_once(end)
        .map(|(matched, _)| matched)
        .ok_or_else(|| eyre!("missing end marker {end:?} after {start:?}"))
}

fn extract_storage_address(line: &str) -> Result<&str> {
    let rest = extract_after(line, "storage_trie{addr=")?;
    let end = rest.find([' ', '}']).ok_or_else(|| eyre!("missing end of storage address"))?;
    Ok(&rest[..end])
}

fn nibble_hex(nibble: u8) -> char {
    char::from_digit(nibble as u32, 16).expect("valid nibble")
}

fn nibbles_hex(path: &Nibbles) -> String {
    let mut out = String::from("0x");
    for nibble in path.iter() {
        out.push(nibble_hex(nibble));
    }
    out
}

fn option_b256(value: Option<B256>) -> String {
    value.map_or_else(|| "None".to_string(), |hash| hash.to_string())
}

trait TraceLine {
    fn first_line(&self) -> usize;
}

impl TraceLine for ObservedStateBranch {
    fn first_line(&self) -> usize {
        self.first_line
    }
}

impl TraceLine for ObservedAccountLeaf {
    fn first_line(&self) -> usize {
        self.first_line
    }
}

impl TraceLine for ObservedStorageBranch {
    fn first_line(&self) -> usize {
        self.first_line
    }
}

impl TraceLine for ObservedStorageLeaf {
    fn first_line(&self) -> usize {
        self.first_line
    }
}

impl TraceLine for ObservedStorageRoot {
    fn first_line(&self) -> usize {
        self.first_line
    }
}

type DbStorageRoot<'a, TX, A> =
    StorageRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
