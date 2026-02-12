use clap::Parser;
use eyre::Context;
use memmap2::Mmap;
use rayon::prelude::*;
use reth_db_api::tables::Tables;
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDB;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::PathBuf,
    time::Instant,
};

const PAGEHDRSZ: usize = 20;
const NODESIZE: usize = 8;
const P_BRANCH: u16 = 0x01;
const P_LEAF: u16 = 0x02;
const P_LARGE: u16 = 0x04;
const P_META: u16 = 0x08;
const P_DUPFIX: u16 = 0x20;
const N_BIG: u8 = 0x01;
const N_TREE: u8 = 0x02;
const N_DUP: u8 = 0x04;
const P_INVALID: u32 = 0xFFFFFFFF;
const UNREFERENCED: u8 = 0xFF;
const DBI_META: u8 = 0xFE;

const META_GEO: usize = 0x14;
const META_FREE_DB: usize = 0x28;
const META_MAIN_DB: usize = 0x58;
const META_TXNID_A: usize = 0x08;
const META_TXNID_B: usize = 0xB0;
const GEO_NOW: usize = 0x0C;
const TREE_ROOT: usize = 0x08;
const TREE_BRANCH_PAGES: usize = 0x0C;
const TREE_LEAF_PAGES: usize = 0x10;
const TREE_LARGE_PAGES: usize = 0x14;
const TREE_ITEMS: usize = 0x20;
const MDBX_MAGIC: u64 = 0x59659DBDEF4C11;

fn u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

#[derive(Debug, Clone)]
struct TreeDescriptor {
    root: u32,
    branch_pages: u32,
    leaf_pages: u32,
    large_pages: u32,
    items: u64,
}

impl TreeDescriptor {
    fn parse(buf: &[u8], off: usize) -> Self {
        Self {
            root: u32_le(buf, off + TREE_ROOT),
            branch_pages: u32_le(buf, off + TREE_BRANCH_PAGES),
            leaf_pages: u32_le(buf, off + TREE_LEAF_PAGES),
            large_pages: u32_le(buf, off + TREE_LARGE_PAGES),
            items: u64_le(buf, off + TREE_ITEMS),
        }
    }

    fn total_pages(&self) -> u64 {
        self.branch_pages as u64 + self.leaf_pages as u64 + self.large_pages as u64
    }
}

#[derive(Debug, Clone)]
struct DBIInfo {
    name: String,
    dbi_index: u8,
    tree: TreeDescriptor,
}

fn page_flags(buf: &[u8], page_off: usize) -> u16 {
    u16_le(buf, page_off + 0x0A)
}

fn page_nkeys(buf: &[u8], page_off: usize) -> usize {
    u16_le(buf, page_off + 0x0C) as usize / 2
}

fn page_overflow_count(buf: &[u8], page_off: usize) -> u32 {
    u32_le(buf, page_off + 0x0C)
}

fn mark(owner_map: &mut [u8], pgno: usize, dbi_index: u8, conflicts: &mut u64) -> bool {
    if pgno >= owner_map.len() {
        return false;
    }
    let prev = owner_map[pgno];
    if prev == UNREFERENCED {
        owner_map[pgno] = dbi_index;
        return true;
    }
    if prev != dbi_index {
        *conflicts += 1;
    }
    false
}


fn walk_tree_collect(
    buf: &[u8],
    ps: usize,
    root_pgno: u32,
    page_count: usize,
    dbi_index: u8,
) -> Vec<(usize, u8)> {
    if root_pgno == P_INVALID || root_pgno as usize >= page_count {
        return Vec::new();
    }

    let mut result: Vec<(usize, u8)> = Vec::new();
    let mut visited = vec![false; page_count];
    let mut stack: Vec<u32> = vec![root_pgno];

    while let Some(pgno) = stack.pop() {
        let pgno_usize = pgno as usize;
        if pgno_usize >= page_count || visited[pgno_usize] {
            continue;
        }
        visited[pgno_usize] = true;
        result.push((pgno_usize, dbi_index));

        let page_off = pgno_usize * ps;
        if page_off + PAGEHDRSZ > buf.len() {
            continue;
        }
        let flags = page_flags(buf, page_off);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let node_abs = page_off + PAGEHDRSZ + node_rel;
                if node_abs + 4 > buf.len() { break; }
                stack.push(u32_le(buf, node_abs));
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let node_abs = page_off + PAGEHDRSZ + node_rel;
                if node_abs + NODESIZE > buf.len() { break; }

                let mn_flags = buf[node_abs + 4];
                let ksize = u16_le(buf, node_abs + 6) as usize;
                let data_off = node_abs + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    if data_off + 4 > buf.len() { continue; }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= page_count { continue; }
                    let ov_page_off = ov_pgno * ps;
                    if ov_page_off + PAGEHDRSZ > buf.len() { continue; }
                    let ov_count = page_overflow_count(buf, ov_page_off) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if op < page_count && !visited[op] {
                            visited[op] = true;
                            result.push((op, dbi_index));
                        }
                    }
                } else if (mn_flags & (N_DUP | N_TREE)) == (N_DUP | N_TREE) {
                    if data_off + 48 > buf.len() { continue; }
                    let sub = TreeDescriptor::parse(buf, data_off);
                    if sub.root != P_INVALID {
                        stack.push(sub.root);
                    }
                }
            }
        } else if flags & P_LARGE != 0 {
            let ov_count = page_overflow_count(buf, page_off) as usize;
            for op in (pgno_usize + 1)..(pgno_usize + ov_count) {
                if op < page_count && !visited[op] {
                    visited[op] = true;
                    result.push((op, dbi_index));
                }
            }
        }
    }

    result
}

fn discover_named_dbis(
    buf: &[u8],
    ps: usize,
    main_root: u32,
    owner_map: &mut [u8],
    conflicts: &mut u64,
    dbi_main: u8,
    dbi_start: u8,
) -> Vec<DBIInfo> {
    if main_root == P_INVALID || main_root as usize >= owner_map.len() {
        return Vec::new();
    }

    let mut named: Vec<DBIInfo> = Vec::new();
    let mut next_index = dbi_start;
    let mut stack: Vec<u32> = vec![main_root];

    while let Some(pgno) = stack.pop() {
        let pgno_usize = pgno as usize;
        if pgno_usize >= owner_map.len() { continue; }
        if !mark(owner_map, pgno_usize, dbi_main, conflicts) { continue; }

        let page_off = pgno_usize * ps;
        if page_off + PAGEHDRSZ > buf.len() { continue; }
        let flags = page_flags(buf, page_off);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let node_abs = page_off + PAGEHDRSZ + node_rel;
                if node_abs + 4 > buf.len() { break; }
                let child_pgno = u32_le(buf, node_abs);
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let node_abs = page_off + PAGEHDRSZ + node_rel;
                if node_abs + NODESIZE > buf.len() { break; }

                let mn_flags = buf[node_abs + 4];
                let ksize = u16_le(buf, node_abs + 6) as usize;
                let data_off = node_abs + NODESIZE + ksize;

                if mn_flags & N_TREE != 0 {
                    let key_off = node_abs + NODESIZE;
                    if key_off + ksize > buf.len() || data_off + 48 > buf.len() { continue; }
                    let name_bytes = &buf[key_off..key_off + ksize];
                    let name = String::from_utf8_lossy(
                        name_bytes.split(|&b| b == 0).next().unwrap_or(name_bytes)
                    ).to_string();
                    let tree = TreeDescriptor::parse(buf, data_off);
                    named.push(DBIInfo { name, dbi_index: next_index, tree });
                    next_index = next_index.saturating_add(1);
                } else if mn_flags & N_BIG != 0 {
                    if data_off + 4 > buf.len() { continue; }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= owner_map.len() { continue; }
                    let ov_page_off = ov_pgno * ps;
                    if ov_page_off + PAGEHDRSZ > buf.len() { continue; }
                    let ov_count = page_overflow_count(buf, ov_page_off) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        mark(owner_map, op, dbi_main, conflicts);
                    }
                }
            }
        }
    }

    named
}

fn mark_free_pages(
    buf: &[u8],
    ps: usize,
    free_root: u32,
    owner_map: &mut [u8],
    conflicts: &mut u64,
    dbi_free: u8,
) -> u64 {
    if free_root == P_INVALID || free_root as usize >= owner_map.len() {
        return 0;
    }

    let mut marked: u64 = 0;
    let mut stack: Vec<u32> = vec![free_root];

    while let Some(pgno) = stack.pop() {
        let pgno_usize = pgno as usize;
        if pgno_usize >= owner_map.len() { continue; }
        if !mark(owner_map, pgno_usize, dbi_free, conflicts) { continue; }
        marked += 1;

        let page_off = pgno_usize * ps;
        if page_off + PAGEHDRSZ > buf.len() { continue; }
        let flags = page_flags(buf, page_off);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let child_pgno = u32_le(buf, page_off + PAGEHDRSZ + node_rel);
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf, page_off);
            for i in 0..nkeys {
                let idx_off = page_off + PAGEHDRSZ + i * 2;
                if idx_off + 2 > buf.len() { break; }
                let node_rel = u16_le(buf, idx_off) as usize;
                let node_abs = page_off + PAGEHDRSZ + node_rel;
                if node_abs + NODESIZE > buf.len() { break; }

                let mn_flags = buf[node_abs + 4];
                let ksize = u16_le(buf, node_abs + 6) as usize;
                let dsize = u32_le(buf, node_abs) as usize;
                let data_off = node_abs + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    if data_off + 4 > buf.len() { continue; }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= owner_map.len() { continue; }
                    let ov_page_off = ov_pgno * ps;
                    if ov_page_off + PAGEHDRSZ > buf.len() { continue; }
                    let ov_count = page_overflow_count(buf, ov_page_off) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if mark(owner_map, op, dbi_free, conflicts) { marked += 1; }
                    }
                    let ov_data_off = ov_pgno * ps + PAGEHDRSZ;
                    if dsize >= 4 && ov_data_off + 4 <= buf.len() {
                        let pnl_count = u32_le(buf, ov_data_off) as usize;
                        for j in 0..pnl_count {
                            let fp_off = ov_data_off + 4 + j * 4;
                            if fp_off + 4 > buf.len() { break; }
                            let fp = u32_le(buf, fp_off) as usize;
                            if fp < owner_map.len() && owner_map[fp] == UNREFERENCED {
                                owner_map[fp] = dbi_free;
                                marked += 1;
                            }
                        }
                    }
                } else {
                    if dsize >= 4 && data_off + 4 <= buf.len() {
                        let pnl_count = u32_le(buf, data_off) as usize;
                        for j in 0..pnl_count {
                            let fp_off = data_off + 4 + j * 4;
                            if fp_off + 4 > buf.len() { break; }
                            let fp = u32_le(buf, fp_off) as usize;
                            if fp < owner_map.len() && owner_map[fp] == UNREFERENCED {
                                owner_map[fp] = dbi_free;
                                marked += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    marked
}

#[derive(Parser, Debug)]
pub struct Command {
    #[arg(short, long, default_value = "owner_map.bin")]
    output: PathBuf,
}

impl Command {
    pub fn execute<N: NodeTypesWithDB>(
        self,
        data_dir: ChainPath<DataDirPath>,
        _tool: &DbTool<N>,
    ) -> eyre::Result<()> {
        let db_path = data_dir.db().join("mdbx.dat");
        eyre::ensure!(db_path.exists(), "mdbx.dat not found at {:?}", db_path);

        let t0 = Instant::now();

        let file = fs::File::open(&db_path)
            .wrap_err_with(|| format!("Failed to open {}", db_path.display()))?;
        let mmap = unsafe { Mmap::map(&file)? };
        let buf: &[u8] = &mmap;

        if buf.len() < PAGEHDRSZ + 0xC0 {
            eyre::bail!("File too small for MDBX meta page");
        }
        let flags0 = u16_le(buf, 0x0A);
        if flags0 & P_META == 0 {
            eyre::bail!("Page 0 missing P_META flag");
        }
        let magic = u64_le(buf, PAGEHDRSZ);
        if (magic >> 8) != MDBX_MAGIC {
            eyre::bail!("MDBX magic mismatch");
        }

        let candidates = [4096usize, 8192, 16384, 32768, 65536];
        let mut ps = 4096usize;
        let geo_now_raw = u32_le(buf, PAGEHDRSZ + META_GEO + GEO_NOW) as usize;
        for &candidate in &candidates {
            let mapped = geo_now_raw * candidate;
            if mapped >= buf.len() / 2 && mapped <= buf.len() * 4 {
                ps = candidate;
                break;
            }
        }

        let mut best_txnid: u64 = 0;
        let mut best_meta_base: usize = 0;
        for pgno in 0..3usize {
            let page_base = pgno * ps;
            let meta_base = page_base + PAGEHDRSZ;
            if meta_base + 0xC0 > buf.len() { continue; }
            let pflags = u16_le(buf, page_base + 0x0A);
            if pflags & P_META == 0 { continue; }
            let m = u64_le(buf, meta_base);
            if (m >> 8) != MDBX_MAGIC { continue; }
            let txnid_a = u64_le(buf, meta_base + META_TXNID_A);
            let txnid_b = u64_le(buf, meta_base + META_TXNID_B);
            if txnid_a == txnid_b && txnid_a > best_txnid {
                best_txnid = txnid_a;
                best_meta_base = meta_base;
            }
        }
        eyre::ensure!(best_txnid > 0, "No valid meta page found");

        let page_count = u32_le(buf, best_meta_base + META_GEO + GEO_NOW) as usize;
        let free_tree = TreeDescriptor::parse(buf, best_meta_base + META_FREE_DB);
        let main_tree = TreeDescriptor::parse(buf, best_meta_base + META_MAIN_DB);

        println!("MDBX owner map generator (parallel)");
        println!("  page_size:  {ps}");
        println!("  page_count: {page_count}");
        println!("  txnid:      {best_txnid}");
        println!("  FreeDB root: {} ({} items)", free_tree.root, free_tree.items);
        println!("  MainDB root: {} ({} items)", main_tree.root, main_tree.items);
        println!();

        let mut owner_map = vec![UNREFERENCED; page_count];
        let mut conflicts: u64 = 0;

        for pgno in 0..std::cmp::min(3, page_count) {
            owner_map[pgno] = DBI_META;
        }

        println!("Discovering named DBIs via MainDB walk...");
        let discovered = discover_named_dbis(
            buf, ps, main_tree.root, &mut owner_map, &mut conflicts, 1, 2,
        );

        let mut name_to_dbi: HashMap<&str, u8> = HashMap::new();
        for (idx, table) in Tables::ALL.iter().enumerate() {
            name_to_dbi.insert(table.name(), (idx + 2) as u8);
        }

        let mut named_dbis: Vec<DBIInfo> = Vec::new();
        let mut remap: HashMap<u8, u8> = HashMap::new();
        for dbi in &discovered {
            if let Some(&real_dbi) = name_to_dbi.get(dbi.name.as_str()) {
                remap.insert(dbi.dbi_index, real_dbi);
                named_dbis.push(DBIInfo {
                    name: dbi.name.clone(),
                    dbi_index: real_dbi,
                    tree: dbi.tree.clone(),
                });
            } else {
                named_dbis.push(dbi.clone());
            }
        }

        for byte in owner_map.iter_mut() {
            if let Some(&new_dbi) = remap.get(byte) {
                *byte = new_dbi;
            }
        }

        println!("Found {} named DBI(s)", named_dbis.len());

        println!("Walking FreeDB / GC...");
        let free_marked = mark_free_pages(buf, ps, free_tree.root, &mut owner_map, &mut conflicts, 0);
        println!("FreeDB: {} pages marked", free_marked);

        println!("Walking {} named DBIs in parallel...", named_dbis.len());
        let t_par = Instant::now();

        let results: Vec<(String, u8, Vec<(usize, u8)>)> = named_dbis
            .par_iter()
            .filter(|dbi| dbi.tree.root != P_INVALID)
            .map(|dbi| {
                let pages = walk_tree_collect(buf, ps, dbi.tree.root, page_count, dbi.dbi_index);
                (dbi.name.clone(), dbi.dbi_index, pages)
            })
            .collect();

        for (name, dbi_index, pages) in &results {
            let count = pages.len();
            for &(pgno, idx) in pages {
                mark(&mut owner_map, pgno, idx, &mut conflicts);
            }
            println!("  [{:2}] {:30} {:>10} pages", dbi_index, name, count);
        }

        println!("Parallel walk done in {:.2}s", t_par.elapsed().as_secs_f64());

        let elapsed = t0.elapsed();

        let mut counts: HashMap<u8, u64> = HashMap::new();
        let mut unreferenced: u64 = 0;
        for &b in &owner_map {
            if b == UNREFERENCED {
                unreferenced += 1;
            } else {
                *counts.entry(b).or_default() += 1;
            }
        }

        println!();
        println!("Walk complete in {:.2}s", elapsed.as_secs_f64());
        println!("  Total pages:  {page_count}");
        println!("  Unreferenced: {unreferenced}");
        println!("  Conflicts:    {conflicts}");
        println!();

        if let Some(&c) = counts.get(&DBI_META) {
            println!("  [0xFE] {:30} {:>10} pages", "<meta>", c);
        }
        if let Some(&c) = counts.get(&0) {
            println!("  [  0] {:30} {:>10} pages", "<free/GC>", c);
        }
        if let Some(&c) = counts.get(&1) {
            println!("  [  1] {:30} {:>10} pages", "<main>", c);
        }
        let mut sorted_dbis = named_dbis.clone();
        sorted_dbis.sort_by_key(|d| d.dbi_index);
        for dbi in &sorted_dbis {
            let walked = counts.get(&dbi.dbi_index).copied().unwrap_or(0);
            let expected = dbi.tree.total_pages();
            let mismatch = if expected > 0 && walked != expected {
                format!("  !! MISMATCH (tree_t says {})", expected)
            } else {
                String::new()
            };
            println!("  [{:3}] {:30} {:>10} pages{}", dbi.dbi_index, dbi.name, walked, mismatch);
        }
        println!("  [0xFF] {:30} {:>10} pages", "<unreferenced>", unreferenced);

        let mut out = fs::File::create(&self.output)?;
        out.write_all(&owner_map)?;
        println!();
        println!("Written {} bytes to {}", owner_map.len(), self.output.display());

        Ok(())
    }
}
