//! Cache-window benchmark: replays a fixed set of captured `BlockAccessedState`
//! fixtures through the `NetworkStateCache` across a grid of
//! `(account_window, storage_window)` policy sizes, and reports
//! **cache size vs hit ratio** — plus a per-category breakdown that pinpoints the
//! hot-spot (accounts vs storage vs code).
//!
//! It is fully offline and deterministic: no node, no EVM, no Merkle proofs. The
//! only input is the fixture directory produced by running the ExEx with
//! `PS_CAPTURE_DIR=<dir>`.
//!
//! Usage:
//!   cache_window_bench [--fixtures <dir>] \
//!                      [--account-windows 8,16,30,60,90,128] \
//!                      [--storage-windows 8,16,30,60,90,128] \
//!                      [--warmup <N>] [--baseline 60,60] [--out <csv>]
//!
//! Defaults: --fixtures ./fixtures/accessed, full cross product of the two window
//! lists, warmup = largest window in the grid (so every config is scored over the
//! identical fully-warmed block range), out = ./cache_window_bench.csv.

use std::{
    collections::BTreeSet,
    error::Error,
    path::{Path, PathBuf},
    process,
};

use partial_stateless::{
    fixture::load_fixtures, network_cache::NetworkStateCache, policy::LastNBlocksPolicy,
};

type Res<T> = Result<T, Box<dyn Error>>;

const DEFAULT_WINDOWS: &[u64] = &[8, 16, 30, 60, 90, 128];

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

struct Args {
    fixtures: PathBuf,
    account_windows: Vec<u64>,
    storage_windows: Vec<u64>,
    warmup: Option<usize>,
    baseline: (u64, u64),
    out: PathBuf,
}

fn run() -> Res<()> {
    let args = parse_args()?;

    let loaded = load_fixtures(&args.fixtures).map_err(|e| {
        format!(
            "failed to read fixtures from {}: {e}\n\
             (capture some first: run the ExEx with PS_CAPTURE_DIR={})",
            args.fixtures.display(),
            args.fixtures.display()
        )
    })?;

    if loaded.fixtures.is_empty() {
        return Err(format!("no fixtures found in {}", args.fixtures.display()).into());
    }
    for (path, err) in &loaded.skipped {
        eprintln!("warning: skipped unreadable fixture {}: {err}", path.display());
    }

    let total = loaded.fixtures.len();
    let (first, last) = loaded.range().unwrap();
    if !loaded.is_contiguous() {
        eprintln!(
            "warning: fixture block range {first}..={last} ({total} files) has gaps — \
             LastN windows are measured in block height, so a gap will distort eviction"
        );
    }

    // Default warmup = the largest window in the grid, so the cache is fully warmed
    // for every config and all configs are scored over the SAME measured blocks.
    let max_window = args
        .account_windows
        .iter()
        .chain(&args.storage_windows)
        .copied()
        .max()
        .unwrap_or(0);
    let warmup = args.warmup.unwrap_or(max_window as usize).min(total.saturating_sub(1));
    let measured = total - warmup;

    println!(
        "Loaded {total} fixtures (blocks {first}..={last}); warmup={warmup}, measured={measured}"
    );
    if warmup < max_window as usize {
        eprintln!(
            "warning: warmup={warmup} < largest window {max_window}; the largest-window \
             configs are not fully warmed, so their hit ratio is pessimistic"
        );
    }
    println!();

    let mut rows: Vec<Row> = Vec::new();
    for &aw in &args.account_windows {
        for &sw in &args.storage_windows {
            rows.push(simulate(&loaded.fixtures, aw, sw, warmup));
        }
    }

    print_table(&rows);
    print_hotspot(&loaded.fixtures, args.baseline, warmup);
    write_csv(&args.out, &rows)?;
    println!("\nWrote {} rows to {}", rows.len(), args.out.display());

    Ok(())
}

/// Aggregate result for one `(account_window, storage_window)` configuration.
struct Row {
    account_window: u64,
    storage_window: u64,
    measured_blocks: usize,
    // accessed / hit totals per category, summed across measured blocks
    acc_accessed: u64,
    acc_hit: u64,
    sto_accessed: u64,
    sto_hit: u64,
    code_accessed: u64,
    code_hit: u64,
    // cache footprint, averaged / peaked over measured blocks
    avg_cache_accounts: f64,
    avg_cache_storage: f64,
    avg_cache_codes: f64,
    avg_cache_mem: f64,
    peak_cache_mem: usize,
}

impl Row {
    fn overall_hit_pct(&self) -> f64 {
        pct(self.acc_hit + self.sto_hit + self.code_hit, self.acc_accessed + self.sto_accessed + self.code_accessed)
    }
    fn account_hit_pct(&self) -> f64 {
        pct(self.acc_hit, self.acc_accessed)
    }
    fn storage_hit_pct(&self) -> f64 {
        pct(self.sto_hit, self.sto_accessed)
    }
    fn code_hit_pct(&self) -> f64 {
        pct(self.code_hit, self.code_accessed)
    }
}

/// Replay all fixtures through a fresh cache; accumulate metrics only for blocks
/// at index >= `warmup` (the cache is still updated during warmup so eviction
/// windows are populated correctly).
fn simulate(fixtures: &[partial_stateless::AccessedStateFixture], aw: u64, sw: u64, warmup: usize) -> Row {
    let mut cache = NetworkStateCache::new(
        Box::new(LastNBlocksPolicy::new(aw)),
        Box::new(LastNBlocksPolicy::new(sw)),
    );

    let mut row = Row {
        account_window: aw,
        storage_window: sw,
        measured_blocks: 0,
        acc_accessed: 0,
        acc_hit: 0,
        sto_accessed: 0,
        sto_hit: 0,
        code_accessed: 0,
        code_hit: 0,
        avg_cache_accounts: 0.0,
        avg_cache_storage: 0.0,
        avg_cache_codes: 0.0,
        avg_cache_mem: 0.0,
        peak_cache_mem: 0,
    };

    let mut sum_cache_accounts = 0u128;
    let mut sum_cache_storage = 0u128;
    let mut sum_cache_codes = 0u128;
    let mut sum_cache_mem = 0u128;

    for (i, fx) in fixtures.iter().enumerate() {
        let measuring = i >= warmup;

        // Miss is computed BEFORE the update — exactly what a validator joining at
        // this block would have to be sent as a witness.
        if measuring {
            let miss = cache.compute_miss(&fx.accessed);
            let a_acc = fx.accessed.accounts.len() as u64;
            let a_sto = fx.accessed.storage.len() as u64;
            let a_code = fx.accessed.codes.len() as u64;
            row.acc_accessed += a_acc;
            row.sto_accessed += a_sto;
            row.code_accessed += a_code;
            row.acc_hit += a_acc - miss.missed_accounts.len() as u64;
            row.sto_hit += a_sto - miss.missed_storage.len() as u64;
            row.code_hit += a_code - miss.missed_codes.len() as u64;
            row.measured_blocks += 1;
        }

        cache.on_block_executed(fx.block_number, &fx.accessed);

        if measuring {
            let snap = cache.snapshot();
            let mem = cache.estimated_memory_bytes();
            sum_cache_accounts += snap.total_accounts as u128;
            sum_cache_storage += snap.total_storage_slots as u128;
            sum_cache_codes += snap.total_codes as u128;
            sum_cache_mem += mem as u128;
            row.peak_cache_mem = row.peak_cache_mem.max(mem);
        }
    }

    let n = row.measured_blocks.max(1) as f64;
    row.avg_cache_accounts = sum_cache_accounts as f64 / n;
    row.avg_cache_storage = sum_cache_storage as f64 / n;
    row.avg_cache_codes = sum_cache_codes as f64 / n;
    row.avg_cache_mem = sum_cache_mem as f64 / n;
    row
}

fn print_table(rows: &[Row]) {
    println!(
        "{:>4} {:>4} | {:>8} {:>8} {:>8} {:>8} | {:>9} {:>9} {:>8} | {:>10}",
        "aw", "sw", "hit%", "acct%", "stor%", "code%", "c.acct", "c.stor", "c.code", "mem"
    );
    println!("{}", "-".repeat(96));
    for r in rows {
        println!(
            "{:>4} {:>4} | {:>7.2}% {:>7.2}% {:>7.2}% {:>7.2}% | {:>9.0} {:>9.0} {:>8.0} | {:>10}",
            r.account_window,
            r.storage_window,
            r.overall_hit_pct(),
            r.account_hit_pct(),
            r.storage_hit_pct(),
            r.code_hit_pct(),
            r.avg_cache_accounts,
            r.avg_cache_storage,
            r.avg_cache_codes,
            human_bytes(r.avg_cache_mem as usize),
        );
    }
}

/// Per-category miss breakdown at the baseline config — answers "which dimension
/// is the hot-spot": the category responsible for the largest share of misses is
/// where extra cache memory buys the most hit ratio.
fn print_hotspot(fixtures: &[partial_stateless::AccessedStateFixture], baseline: (u64, u64), warmup: usize) {
    let (aw, sw) = baseline;
    let r = simulate(fixtures, aw, sw, warmup);

    let acc_miss = r.acc_accessed - r.acc_hit;
    let sto_miss = r.sto_accessed - r.sto_hit;
    let code_miss = r.code_accessed - r.code_hit;
    let total_miss = (acc_miss + sto_miss + code_miss).max(1);

    println!("\n── Hot-spot @ baseline (account_window={aw}, storage_window={sw}) ──");
    println!(
        "  total misses/block: {:.1}   (accounts {:.1} + storage {:.1} + code {:.1})",
        total_miss as f64 / r.measured_blocks.max(1) as f64,
        acc_miss as f64 / r.measured_blocks.max(1) as f64,
        sto_miss as f64 / r.measured_blocks.max(1) as f64,
        code_miss as f64 / r.measured_blocks.max(1) as f64,
    );
    println!(
        "  miss share:  accounts {:>5.1}%   storage {:>5.1}%   code {:>5.1}%",
        pct(acc_miss, total_miss),
        pct(sto_miss, total_miss),
        pct(code_miss, total_miss),
    );
    let (label, _) = [("accounts", acc_miss), ("storage", sto_miss), ("code", code_miss)]
        .into_iter()
        .max_by_key(|(_, m)| *m)
        .unwrap();
    println!("  => dominant witness contributor: {label}");
}

fn write_csv(path: &Path, rows: &[Row]) -> Res<()> {
    let mut out = String::from(
        "account_window,storage_window,measured_blocks,overall_hit_pct,account_hit_pct,\
         storage_hit_pct,code_hit_pct,avg_cache_accounts,avg_cache_storage,avg_cache_codes,\
         avg_cache_mem_bytes,peak_cache_mem_bytes\n",
    );
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{:.4},{:.4},{:.4},{:.4},{:.1},{:.1},{:.1},{:.0},{}\n",
            r.account_window,
            r.storage_window,
            r.measured_blocks,
            r.overall_hit_pct(),
            r.account_hit_pct(),
            r.storage_hit_pct(),
            r.code_hit_pct(),
            r.avg_cache_accounts,
            r.avg_cache_storage,
            r.avg_cache_codes,
            r.avg_cache_mem,
            r.peak_cache_mem,
        ));
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn pct(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64 * 100.0
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn parse_args() -> Res<Args> {
    let mut fixtures = PathBuf::from("./fixtures/accessed");
    let mut account_windows: Option<Vec<u64>> = None;
    let mut storage_windows: Option<Vec<u64>> = None;
    let mut warmup: Option<usize> = None;
    let mut baseline = (60u64, 60u64);
    let mut out = PathBuf::from("./cache_window_bench.csv");

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--fixtures" => fixtures = PathBuf::from(next(&mut it, "--fixtures")?),
            "--account-windows" => account_windows = Some(parse_u64_list(&next(&mut it, "--account-windows")?)?),
            "--storage-windows" => storage_windows = Some(parse_u64_list(&next(&mut it, "--storage-windows")?)?),
            "--warmup" => warmup = Some(next(&mut it, "--warmup")?.parse()?),
            "--baseline" => {
                let v = parse_u64_list(&next(&mut it, "--baseline")?)?;
                if v.len() != 2 {
                    return Err("--baseline expects two comma-separated values, e.g. 60,60".into());
                }
                baseline = (v[0], v[1]);
            }
            "--out" => out = PathBuf::from(next(&mut it, "--out")?),
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            other => return Err(format!("unknown argument: {other} (try --help)").into()),
        }
    }

    // Dedup + sort so the grid is stable regardless of input order.
    let dedup = |v: Vec<u64>| v.into_iter().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
    Ok(Args {
        fixtures,
        account_windows: dedup(account_windows.unwrap_or_else(|| DEFAULT_WINDOWS.to_vec())),
        storage_windows: dedup(storage_windows.unwrap_or_else(|| DEFAULT_WINDOWS.to_vec())),
        warmup,
        baseline,
        out,
    })
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> Res<String> {
    it.next().ok_or_else(|| format!("missing value for {flag}").into())
}

fn parse_u64_list(s: &str) -> Res<Vec<u64>> {
    s.split(',')
        .map(|p| p.trim().parse::<u64>().map_err(|e| format!("bad number '{p}': {e}").into()))
        .collect()
}

fn print_help() {
    println!(
        "cache_window_bench — sweep cache window vs hit ratio over captured fixtures\n\n\
         OPTIONS:\n\
         \x20 --fixtures <dir>          fixture dir (default ./fixtures/accessed)\n\
         \x20 --account-windows a,b,c   account LastN windows (default 8,16,30,60,90,128)\n\
         \x20 --storage-windows a,b,c   storage/code LastN windows (default 8,16,30,60,90,128)\n\
         \x20 --warmup <N>              blocks to warm before scoring (default = max window)\n\
         \x20 --baseline aw,sw          config used for the hot-spot breakdown (default 60,60)\n\
         \x20 --out <path>              CSV output (default ./cache_window_bench.csv)\n"
    );
}
