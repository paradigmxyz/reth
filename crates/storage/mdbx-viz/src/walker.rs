use rayon::prelude::*;
use serde::Serialize;
use std::{collections::HashMap, fs::File, io, os::unix::fs::FileExt, path::Path, time::Instant};

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

#[derive(Debug, Clone, Serialize)]
pub struct TreeInfo {
    pub name: String,
    pub dbi_index: u8,
    pub height: u16,
    pub branch_pages: u32,
    pub leaf_pages: u32,
    pub large_pages: u32,
    pub items: u64,
    pub root_pgno: u32,
}

pub struct WalkResult {
    pub owner_map: Vec<u8>,
    pub page_count: usize,
    pub page_size: usize,
    pub tree_info: Vec<TreeInfo>,
}

#[allow(dead_code)]
struct TreeDescriptor {
    flags: u16,
    height: u16,
    dupfix_size: u32,
    root: u32,
    branch_pages: u32,
    leaf_pages: u32,
    large_pages: u32,
    sequence: u64,
    items: u64,
    mod_txnid: u64,
}

struct DBIInfo {
    name: String,
    dbi_index: u8,
    tree: TreeDescriptor,
}

fn u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
        buf[off + 4],
        buf[off + 5],
        buf[off + 6],
        buf[off + 7],
    ])
}

fn page_flags(buf: &[u8]) -> u16 {
    u16_le(buf, 0x0A)
}

fn page_nkeys(buf: &[u8]) -> usize {
    u16_le(buf, 0x0C) as usize / 2
}

fn page_overflow_count(buf: &[u8]) -> u32 {
    u32_le(buf, 0x0C)
}

fn parse_tree_descriptor(buf: &[u8], off: usize) -> TreeDescriptor {
    TreeDescriptor {
        flags: u16_le(buf, off),
        height: u16_le(buf, off + 0x02),
        dupfix_size: u32_le(buf, off + 0x04),
        root: u32_le(buf, off + TREE_ROOT),
        branch_pages: u32_le(buf, off + TREE_BRANCH_PAGES),
        leaf_pages: u32_le(buf, off + TREE_LEAF_PAGES),
        large_pages: u32_le(buf, off + TREE_LARGE_PAGES),
        sequence: u64_le(buf, off + 0x18),
        items: u64_le(buf, off + TREE_ITEMS),
        mod_txnid: u64_le(buf, off + 0x28),
    }
}

fn read_page(file: &File, pgno: usize, ps: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; ps];
    file.read_exact_at(&mut buf, (pgno * ps) as u64)?;
    Ok(buf)
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
    file: &File,
    ps: usize,
    root_pgno: u32,
    page_count: usize,
    dbi_index: u8,
) -> Vec<(usize, u8)> {
    let mut result = Vec::new();
    if root_pgno == P_INVALID || root_pgno as usize >= page_count {
        return result;
    }

    let mut visited = vec![false; page_count];
    let mut stack: Vec<usize> = vec![root_pgno as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= page_count || visited[pgno] {
            continue;
        }
        visited[pgno] = true;
        result.push((pgno, dbi_index));

        let buf = match read_page(file, pgno, ps) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let flags = page_flags(&buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(&buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(&buf, node_off + 6) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    let ov_pgno = u32_le(&buf, data_off) as usize;
                    if ov_pgno >= page_count {
                        continue;
                    }
                    let ov_buf = match read_page(file, ov_pgno, ps) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let ov_count = page_overflow_count(&ov_buf) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if op < page_count && !visited[op] {
                            visited[op] = true;
                            result.push((op, dbi_index));
                        }
                    }
                } else if (mn_flags & (N_DUP | N_TREE)) == (N_DUP | N_TREE) {
                    let sub_root = u32_le(&buf, data_off + TREE_ROOT);
                    if sub_root != P_INVALID {
                        stack.push(sub_root as usize);
                    }
                }
            }
        } else if flags & P_LARGE != 0 {
            let ov_count = page_overflow_count(&buf) as usize;
            for op in (pgno + 1)..pgno + ov_count {
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
    file: &File,
    ps: usize,
    main_root: u32,
    owner_map: &mut [u8],
    conflicts: &mut u64,
    dbi_main: u8,
    dbi_start: u8,
) -> Vec<DBIInfo> {
    let mut named = Vec::new();
    if main_root == P_INVALID || main_root as usize >= owner_map.len() {
        return named;
    }

    let mut next_index = dbi_start;
    let mut stack: Vec<usize> = vec![main_root as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= owner_map.len() {
            continue;
        }
        if !mark(owner_map, pgno, dbi_main, conflicts) {
            continue;
        }

        let buf = match read_page(file, pgno, ps) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let flags = page_flags(&buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(&buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(&buf, node_off + 6) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_TREE != 0 {
                    let key_off = node_off + NODESIZE;
                    let key_bytes = &buf[key_off..key_off + ksize];
                    let name = match key_bytes.iter().position(|&b| b == 0) {
                        Some(nul) => &key_bytes[..nul],
                        None => key_bytes,
                    };
                    let name = String::from_utf8_lossy(name).into_owned();
                    let tree = parse_tree_descriptor(&buf, data_off);
                    named.push(DBIInfo { name, dbi_index: next_index, tree });
                    next_index = next_index.wrapping_add(1);
                } else if mn_flags & N_BIG != 0 {
                    let ov_pgno = u32_le(&buf, data_off) as usize;
                    if ov_pgno >= owner_map.len() {
                        continue;
                    }
                    let ov_buf = match read_page(file, ov_pgno, ps) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let ov_count = page_overflow_count(&ov_buf) as usize;
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
    file: &File,
    ps: usize,
    free_root: u32,
    owner_map: &mut [u8],
    conflicts: &mut u64,
    dbi_free: u8,
) -> u64 {
    let mut marked: u64 = 0;
    if free_root == P_INVALID || free_root as usize >= owner_map.len() {
        return marked;
    }

    let mut stack: Vec<usize> = vec![free_root as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= owner_map.len() {
            continue;
        }
        if !mark(owner_map, pgno, dbi_free, conflicts) {
            continue;
        }
        marked += 1;

        let buf = match read_page(file, pgno, ps) {
            Ok(b) => b,
            Err(_) => continue,
        };

        let flags = page_flags(&buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(&buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(&buf);
            for i in 0..nkeys {
                let node_rel = u16_le(&buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(&buf, node_off + 6) as usize;
                let dsize = u32_le(&buf, node_off) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    let ov_pgno = u32_le(&buf, data_off) as usize;
                    if ov_pgno >= owner_map.len() {
                        continue;
                    }
                    let ov_buf = match read_page(file, ov_pgno, ps) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let ov_count = page_overflow_count(&ov_buf) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if mark(owner_map, op, dbi_free, conflicts) {
                            marked += 1;
                        }
                    }
                    let ov_data_off = PAGEHDRSZ;
                    if dsize >= 4 {
                        let pnl_count = u32_le(&ov_buf, ov_data_off) as usize;
                        for j in 0..pnl_count {
                            let fp = u32_le(&ov_buf, ov_data_off + 4 + j * 4) as usize;
                            if fp < owner_map.len() && owner_map[fp] == UNREFERENCED {
                                owner_map[fp] = dbi_free;
                                marked += 1;
                            }
                        }
                    }
                } else {
                    if dsize >= 4 {
                        let pnl_count = u32_le(&buf, data_off) as usize;
                        for j in 0..pnl_count {
                            let fp = u32_le(&buf, data_off + 4 + j * 4) as usize;
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

pub fn walk_mdbx(path: &Path, name_to_dbi: &HashMap<&str, u8>) -> eyre::Result<WalkResult> {
    let t0 = Instant::now();
    let file = File::open(path)?;
    let file_size = file.metadata()?.len() as usize;

    let meta0 = read_page(&file, 0, 4096)?;
    let pflags = page_flags(&meta0);
    eyre::ensure!(pflags & P_META != 0, "page 0 missing P_META flag");

    let magic = u64_le(&meta0, PAGEHDRSZ);
    eyre::ensure!((magic >> 8) == MDBX_MAGIC, "MDBX magic mismatch");

    let candidates: &[usize] = &[4096, 8192, 16384, 32768, 65536, 1024, 2048];
    let geo_now_raw = u32_le(&meta0, PAGEHDRSZ + META_GEO + GEO_NOW) as usize;

    let mut ps = 4096usize;
    for &candidate in candidates {
        let mapped = geo_now_raw * candidate;
        if mapped >= file_size / 2 && mapped <= file_size * 4 {
            ps = candidate;
            break;
        }
    }

    tracing::info!(page_size = ps, "detected page size");

    let meta_pages: Vec<Vec<u8>> = (0..3)
        .map(|i| read_page(&file, i, ps))
        .collect::<io::Result<Vec<_>>>()?;

    let mut best_idx = 0usize;
    let mut best_txnid = 0u64;

    for (i, meta) in meta_pages.iter().enumerate() {
        let meta_body = PAGEHDRSZ;
        let m = u64_le(meta, meta_body);
        if (m >> 8) != MDBX_MAGIC {
            continue;
        }
        let txnid_a = u64_le(meta, meta_body + META_TXNID_A);
        let txnid_b = u64_le(meta, meta_body + META_TXNID_B);
        if txnid_a == txnid_b && txnid_a > best_txnid {
            best_txnid = txnid_a;
            best_idx = i;
        }
    }

    eyre::ensure!(best_txnid > 0, "no valid meta page found");
    tracing::info!(best_meta = best_idx, txnid = best_txnid, "selected best meta");

    let best = &meta_pages[best_idx];
    let mb = PAGEHDRSZ;

    let geo_now = u32_le(best, mb + META_GEO + GEO_NOW) as usize;
    let page_count = geo_now;

    let free_tree = parse_tree_descriptor(best, mb + META_FREE_DB);
    let main_tree = parse_tree_descriptor(best, mb + META_MAIN_DB);

    tracing::info!(
        page_count,
        free_root = free_tree.root,
        main_root = main_tree.root,
        "parsed meta"
    );

    let mut owner_map = vec![UNREFERENCED; page_count];
    let mut conflicts = 0u64;

    for pgno in 0..3.min(page_count) {
        owner_map[pgno] = DBI_META;
    }

    let dbi_free: u8 = 1;
    let dbi_main: u8 = 2;
    let dbi_start: u8 = 3;

    tracing::info!("discovering named DBIs via MainDB walk");
    let mut named_dbis =
        discover_named_dbis(&file, ps, main_tree.root, &mut owner_map, &mut conflicts, dbi_main, dbi_start);
    tracing::info!(count = named_dbis.len(), "found named DBIs");

    for dbi in &mut named_dbis {
        if let Some(&mapped_idx) = name_to_dbi.get(dbi.name.as_str()) {
            dbi.dbi_index = mapped_idx;
        }
    }

    tracing::info!("walking FreeDB");
    let free_marked =
        mark_free_pages(&file, ps, free_tree.root, &mut owner_map, &mut conflicts, dbi_free);
    tracing::info!(marked = free_marked, "FreeDB walk complete");

    let walk_tasks: Vec<(u32, u8)> = named_dbis
        .iter()
        .filter(|d| d.tree.root != P_INVALID)
        .map(|d| (d.tree.root, d.dbi_index))
        .collect();

    tracing::info!(count = walk_tasks.len(), "walking named DBIs in parallel");

    let parallel_results: Vec<Vec<(usize, u8)>> = walk_tasks
        .par_iter()
        .map(|&(root, dbi_idx)| walk_tree_collect(&file, ps, root, page_count, dbi_idx))
        .collect();

    for batch in parallel_results {
        for (pgno, dbi_idx) in batch {
            mark(&mut owner_map, pgno, dbi_idx, &mut conflicts);
        }
    }

    let elapsed = t0.elapsed();
    let assigned = owner_map.iter().filter(|&&b| b != UNREFERENCED).count();
    tracing::info!(
        elapsed_ms = elapsed.as_millis() as u64,
        assigned,
        unreferenced = page_count - assigned,
        conflicts,
        "walk complete"
    );

    let mut tree_info = Vec::new();

    tree_info.push(TreeInfo {
        name: "FreeDB".to_string(),
        dbi_index: dbi_free,
        height: free_tree.height,
        branch_pages: free_tree.branch_pages,
        leaf_pages: free_tree.leaf_pages,
        large_pages: free_tree.large_pages,
        items: free_tree.items,
        root_pgno: free_tree.root,
    });

    tree_info.push(TreeInfo {
        name: "MainDB".to_string(),
        dbi_index: dbi_main,
        height: main_tree.height,
        branch_pages: main_tree.branch_pages,
        leaf_pages: main_tree.leaf_pages,
        large_pages: main_tree.large_pages,
        items: main_tree.items,
        root_pgno: main_tree.root,
    });

    for dbi in &named_dbis {
        tree_info.push(TreeInfo {
            name: dbi.name.clone(),
            dbi_index: dbi.dbi_index,
            height: dbi.tree.height,
            branch_pages: dbi.tree.branch_pages,
            leaf_pages: dbi.tree.leaf_pages,
            large_pages: dbi.tree.large_pages,
            items: dbi.tree.items,
            root_pgno: dbi.tree.root,
        });
    }

    Ok(WalkResult { owner_map, page_count, page_size: ps, tree_info })
}
