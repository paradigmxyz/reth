use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    os::unix::io::AsRawFd,
    path::Path,
    sync::atomic::{AtomicU8, AtomicU64, Ordering},
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

const CACHE_MAGIC: u64 = 0x5056495A_43414348; // "PVIZC ACH"
const CACHE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
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

struct MmapFile {
    ptr: *mut u8,
    len: usize,
}

impl MmapFile {
    fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty file"));
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr: ptr as *mut u8, len })
    }

    #[inline]
    fn page(&self, pgno: usize, ps: usize) -> Option<&[u8]> {
        let off = pgno.checked_mul(ps)?;
        if off + ps > self.len {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(self.ptr.add(off), ps) })
    }

    fn evict(&self) {
        unsafe {
            libc::madvise(self.ptr as *mut libc::c_void, self.len, libc::MADV_DONTNEED);
        }
        tracing::info!(size_gb = self.len / (1024 * 1024 * 1024), "evicted walker mmap from page cache");
    }
}

impl Drop for MmapFile {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut libc::c_void, self.len); }
    }
}

unsafe impl Send for MmapFile {}
unsafe impl Sync for MmapFile {}

#[inline]
fn u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

#[inline]
fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

#[inline]
fn u64_le(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        buf[off], buf[off + 1], buf[off + 2], buf[off + 3],
        buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7],
    ])
}

#[inline]
fn page_flags(buf: &[u8]) -> u16 {
    u16_le(buf, 0x0A)
}

#[inline]
fn page_nkeys(buf: &[u8]) -> usize {
    u16_le(buf, 0x0C) as usize / 2
}

#[inline]
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

#[inline]
fn claim(owner_map: &[AtomicU8], pgno: usize, dbi_index: u8, conflicts: &AtomicU64) -> bool {
    if pgno >= owner_map.len() {
        return false;
    }
    match owner_map[pgno].compare_exchange(UNREFERENCED, dbi_index, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => true,
        Err(existing) => {
            if existing != dbi_index {
                conflicts.fetch_add(1, Ordering::Relaxed);
            }
            false
        }
    }
}

fn walk_tree_claim(
    mmap: &MmapFile,
    ps: usize,
    root_pgno: u32,
    page_count: usize,
    dbi_index: u8,
    owner_map: &[AtomicU8],
    conflicts: &AtomicU64,
) {
    if root_pgno == P_INVALID || root_pgno as usize >= page_count {
        return;
    }

    let mut stack: Vec<usize> = vec![root_pgno as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= page_count {
            continue;
        }
        if !claim(owner_map, pgno, dbi_index, conflicts) {
            continue;
        }

        let buf = match mmap.page(pgno, ps) {
            Some(b) => b,
            None => continue,
        };

        let flags = page_flags(buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                if node_off + NODESIZE > ps {
                    continue;
                }
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(buf, node_off + 6) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    if data_off + 4 > ps {
                        continue;
                    }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= page_count {
                        continue;
                    }
                    let ov_buf = match mmap.page(ov_pgno, ps) {
                        Some(b) => b,
                        None => continue,
                    };
                    let ov_count = page_overflow_count(ov_buf) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if op < page_count {
                            claim(owner_map, op, dbi_index, conflicts);
                        }
                    }
                } else if (mn_flags & (N_DUP | N_TREE)) == (N_DUP | N_TREE) {
                    if data_off + TREE_ROOT + 4 > ps {
                        continue;
                    }
                    let sub_root = u32_le(buf, data_off + TREE_ROOT);
                    if sub_root != P_INVALID {
                        stack.push(sub_root as usize);
                    }
                }
            }
        } else if flags & P_LARGE != 0 {
            let ov_count = page_overflow_count(buf) as usize;
            for op in (pgno + 1)..pgno + ov_count {
                if op < page_count {
                    claim(owner_map, op, dbi_index, conflicts);
                }
            }
        }
    }
}

fn discover_named_dbis(
    mmap: &MmapFile,
    ps: usize,
    main_root: u32,
    page_count: usize,
    owner_map: &[AtomicU8],
    conflicts: &AtomicU64,
    dbi_main: u8,
    dbi_start: u8,
) -> Vec<DBIInfo> {
    let mut named = Vec::new();
    if main_root == P_INVALID || main_root as usize >= page_count {
        return named;
    }

    let mut next_index = dbi_start;
    let mut stack: Vec<usize> = vec![main_root as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= page_count {
            continue;
        }
        if !claim(owner_map, pgno, dbi_main, conflicts) {
            continue;
        }

        let buf = match mmap.page(pgno, ps) {
            Some(b) => b,
            None => continue,
        };

        let flags = page_flags(buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                if node_off + NODESIZE > ps {
                    continue;
                }
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(buf, node_off + 6) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_TREE != 0 {
                    let key_off = node_off + NODESIZE;
                    if key_off + ksize > ps || data_off + 0x30 > ps {
                        continue;
                    }
                    let key_bytes = &buf[key_off..key_off + ksize];
                    let name = match key_bytes.iter().position(|&b| b == 0) {
                        Some(nul) => &key_bytes[..nul],
                        None => key_bytes,
                    };
                    let name = String::from_utf8_lossy(name).into_owned();
                    let tree = parse_tree_descriptor(buf, data_off);
                    named.push(DBIInfo { name, dbi_index: next_index, tree });
                    next_index = next_index.wrapping_add(1);
                } else if mn_flags & N_BIG != 0 {
                    if data_off + 4 > ps {
                        continue;
                    }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= page_count {
                        continue;
                    }
                    let ov_buf = match mmap.page(ov_pgno, ps) {
                        Some(b) => b,
                        None => continue,
                    };
                    let ov_count = page_overflow_count(ov_buf) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        claim(owner_map, op, dbi_main, conflicts);
                    }
                }
            }
        }
    }

    named
}

fn mark_free_pages(
    mmap: &MmapFile,
    ps: usize,
    free_root: u32,
    page_count: usize,
    owner_map: &[AtomicU8],
    conflicts: &AtomicU64,
    dbi_free: u8,
) -> u64 {
    let mut marked: u64 = 0;
    if free_root == P_INVALID || free_root as usize >= page_count {
        return marked;
    }

    let mut stack: Vec<usize> = vec![free_root as usize];

    while let Some(pgno) = stack.pop() {
        if pgno >= page_count {
            continue;
        }
        if !claim(owner_map, pgno, dbi_free, conflicts) {
            continue;
        }
        marked += 1;

        let buf = match mmap.page(pgno, ps) {
            Some(b) => b,
            None => continue,
        };

        let flags = page_flags(buf);

        if flags & P_BRANCH != 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let child_pgno = u32_le(buf, PAGEHDRSZ + node_rel) as usize;
                stack.push(child_pgno);
            }
        } else if flags & P_LEAF != 0 && flags & P_DUPFIX == 0 {
            let nkeys = page_nkeys(buf);
            for i in 0..nkeys {
                let node_rel = u16_le(buf, PAGEHDRSZ + i * 2) as usize;
                let node_off = PAGEHDRSZ + node_rel;
                if node_off + NODESIZE > ps {
                    continue;
                }
                let mn_flags = buf[node_off + 4];
                let ksize = u16_le(buf, node_off + 6) as usize;
                let dsize = u32_le(buf, node_off) as usize;
                let data_off = node_off + NODESIZE + ksize;

                if mn_flags & N_BIG != 0 {
                    if data_off + 4 > ps {
                        continue;
                    }
                    let ov_pgno = u32_le(buf, data_off) as usize;
                    if ov_pgno >= page_count {
                        continue;
                    }
                    let ov_buf = match mmap.page(ov_pgno, ps) {
                        Some(b) => b,
                        None => continue,
                    };
                    let ov_count = page_overflow_count(ov_buf) as usize;
                    for op in ov_pgno..ov_pgno + ov_count {
                        if claim(owner_map, op, dbi_free, conflicts) {
                            marked += 1;
                        }
                    }
                    let ov_data_off = PAGEHDRSZ;
                    if dsize >= 4 && ov_data_off + 4 <= ps {
                        let pnl_count = u32_le(ov_buf, ov_data_off) as usize;
                        let max_entries = (ps - ov_data_off - 4) / 4;
                        let pnl_count = pnl_count.min(max_entries);
                        for j in 0..pnl_count {
                            let off = ov_data_off + 4 + j * 4;
                            if off + 4 > ps { break; }
                            let fp = u32_le(ov_buf, off) as usize;
                            if fp < page_count {
                                if claim(owner_map, fp, dbi_free, conflicts) {
                                    marked += 1;
                                }
                            }
                        }
                    }
                } else {
                    if dsize >= 4 && data_off + 4 <= ps {
                        let pnl_count = u32_le(buf, data_off) as usize;
                        let max_entries = (ps - data_off - 4) / 4;
                        let pnl_count = pnl_count.min(max_entries);
                        for j in 0..pnl_count {
                            let off = data_off + 4 + j * 4;
                            if off + 4 > ps { break; }
                            let fp = u32_le(buf, off) as usize;
                            if fp < page_count {
                                if claim(owner_map, fp, dbi_free, conflicts) {
                                    marked += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    marked
}

fn cache_path(mdbx_path: &Path) -> std::path::PathBuf {
    mdbx_path.with_file_name("mdbx_pageviz_cache.bin")
}

fn try_load_cache(
    path: &Path,
    file_size: u64,
    txnid: u64,
    page_size: usize,
    page_count: usize,
) -> Option<WalkResult> {
    let cp = cache_path(path);
    let mut f = std::fs::File::open(&cp).ok()?;
    let mut hdr = [0u8; 40];
    f.read_exact(&mut hdr).ok()?;

    let magic = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
    let ver = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    let c_txnid = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
    let c_pc = u64::from_le_bytes(hdr[20..28].try_into().unwrap());
    let c_ps = u32::from_le_bytes(hdr[28..32].try_into().unwrap());
    let c_fsz = u64::from_le_bytes(hdr[32..40].try_into().unwrap());

    if magic != CACHE_MAGIC || ver != CACHE_VERSION {
        return None;
    }
    if c_txnid != txnid || c_pc as usize != page_count || c_ps as usize != page_size || c_fsz != file_size {
        tracing::info!(
            cache_txnid = c_txnid, current_txnid = txnid,
            "cache stale, will re-walk"
        );
        return None;
    }

    let mut ti_len_buf = [0u8; 4];
    f.read_exact(&mut ti_len_buf).ok()?;
    let ti_len = u32::from_le_bytes(ti_len_buf) as usize;

    let mut ti_buf = vec![0u8; ti_len];
    f.read_exact(&mut ti_buf).ok()?;
    let tree_info: Vec<TreeInfo> = serde_json::from_slice(&ti_buf).ok()?;

    let mut owner_map = vec![0u8; page_count];
    f.read_exact(&mut owner_map).ok()?;

    tracing::info!(page_count, "loaded owner_map from cache");
    Some(WalkResult { owner_map, page_count, page_size, tree_info })
}

fn save_cache(
    path: &Path,
    file_size: u64,
    txnid: u64,
    result: &WalkResult,
) {
    let cp = cache_path(path);
    let tmp = cp.with_extension("tmp");

    let write_inner = || -> io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&CACHE_MAGIC.to_le_bytes())?;
        f.write_all(&CACHE_VERSION.to_le_bytes())?;
        f.write_all(&txnid.to_le_bytes())?;
        f.write_all(&(result.page_count as u64).to_le_bytes())?;
        f.write_all(&(result.page_size as u32).to_le_bytes())?;
        f.write_all(&file_size.to_le_bytes())?;

        let ti_json = serde_json::to_vec(&result.tree_info).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        f.write_all(&(ti_json.len() as u32).to_le_bytes())?;
        f.write_all(&ti_json)?;
        f.write_all(&result.owner_map)?;
        f.flush()?;
        Ok(())
    };

    match write_inner() {
        Ok(()) => {
            if let Err(e) = std::fs::rename(&tmp, &cp) {
                tracing::warn!("failed to rename cache file: {e}");
                let _ = std::fs::remove_file(&tmp);
            } else {
                tracing::info!(
                    path = %cp.display(),
                    size_mb = result.owner_map.len() / (1024 * 1024),
                    "saved owner_map cache"
                );
            }
        }
        Err(e) => {
            tracing::warn!("failed to write cache: {e}");
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

pub fn walk_mdbx(path: &Path, name_to_dbi: &HashMap<&str, u8>) -> eyre::Result<WalkResult> {
    let t0 = Instant::now();

    let mmap = MmapFile::open(path)?;
    let file_size = mmap.len as u64;

    let meta0 = mmap.page(0, 4096).ok_or_else(|| eyre::eyre!("cannot read page 0"))?;
    let pflags = page_flags(meta0);
    eyre::ensure!(pflags & P_META != 0, "page 0 missing P_META flag");

    let magic = u64_le(meta0, PAGEHDRSZ);
    eyre::ensure!((magic >> 8) == MDBX_MAGIC, "MDBX magic mismatch");

    let candidates: &[usize] = &[4096, 8192, 16384, 32768, 65536, 1024, 2048];
    let geo_now_raw = u32_le(meta0, PAGEHDRSZ + META_GEO + GEO_NOW) as usize;

    let mut ps = 4096usize;
    for &candidate in candidates {
        let mapped = geo_now_raw * candidate;
        if mapped >= mmap.len / 2 && mapped <= mmap.len * 4 {
            ps = candidate;
            break;
        }
    }

    tracing::info!(page_size = ps, file_size_gb = file_size / (1024 * 1024 * 1024), "detected page size");

    let meta_pages: Vec<&[u8]> = (0..3)
        .map(|i| mmap.page(i, ps).ok_or_else(|| eyre::eyre!("cannot read meta page {i}")))
        .collect::<eyre::Result<Vec<_>>>()?;

    let mut best_idx = 0usize;
    let mut best_txnid = 0u64;

    for (i, meta) in meta_pages.iter().enumerate() {
        let m = u64_le(meta, PAGEHDRSZ);
        if (m >> 8) != MDBX_MAGIC {
            continue;
        }
        let txnid_a = u64_le(meta, PAGEHDRSZ + META_TXNID_A);
        let txnid_b = u64_le(meta, PAGEHDRSZ + META_TXNID_B);
        if txnid_a == txnid_b && txnid_a > best_txnid {
            best_txnid = txnid_a;
            best_idx = i;
        }
    }

    eyre::ensure!(best_txnid > 0, "no valid meta page found");

    let best = meta_pages[best_idx];
    let mb = PAGEHDRSZ;
    let geo_now = u32_le(best, mb + META_GEO + GEO_NOW) as usize;
    let page_count = geo_now;

    tracing::info!(best_meta = best_idx, txnid = best_txnid, page_count, "selected best meta");

    if let Some(cached) = try_load_cache(path, file_size, best_txnid, ps, page_count) {
        let elapsed = t0.elapsed();
        tracing::info!(elapsed_ms = elapsed.as_millis() as u64, "walk skipped (cache hit)");
        return Ok(cached);
    }

    let free_tree = parse_tree_descriptor(best, mb + META_FREE_DB);
    let main_tree = parse_tree_descriptor(best, mb + META_MAIN_DB);

    tracing::info!(
        page_count,
        free_root = free_tree.root,
        main_root = main_tree.root,
        "parsed meta, starting walk"
    );

    let owner_map: Vec<AtomicU8> = (0..page_count).map(|_| AtomicU8::new(UNREFERENCED)).collect();
    let conflicts = AtomicU64::new(0);

    for pgno in 0..3.min(page_count) {
        owner_map[pgno].store(DBI_META, Ordering::Relaxed);
    }

    let dbi_free: u8 = 1;
    let dbi_main: u8 = 2;
    let dbi_start: u8 = 3;

    tracing::info!("discovering named DBIs via MainDB walk");
    let mut named_dbis =
        discover_named_dbis(&mmap, ps, main_tree.root, page_count, &owner_map, &conflicts, dbi_main, dbi_start);
    tracing::info!(count = named_dbis.len(), "found named DBIs");

    for dbi in &mut named_dbis {
        if let Some(&mapped_idx) = name_to_dbi.get(dbi.name.as_str()) {
            dbi.dbi_index = mapped_idx;
        }
    }

    tracing::info!("walking FreeDB");
    let free_marked =
        mark_free_pages(&mmap, ps, free_tree.root, page_count, &owner_map, &conflicts, dbi_free);
    tracing::info!(marked = free_marked, "FreeDB walk complete");

    let walk_tasks: Vec<(u32, u8)> = named_dbis
        .iter()
        .filter(|d| d.tree.root != P_INVALID)
        .map(|d| (d.tree.root, d.dbi_index))
        .collect();

    tracing::info!(count = walk_tasks.len(), "walking named DBIs in parallel");

    walk_tasks
        .par_iter()
        .for_each(|&(root, dbi_idx)| {
            walk_tree_claim(&mmap, ps, root, page_count, dbi_idx, &owner_map, &conflicts);
        });

    let final_conflicts = conflicts.load(Ordering::Relaxed);
    let owner_map_vec: Vec<u8> = owner_map.iter().map(|a| a.load(Ordering::Relaxed)).collect();
    let assigned = owner_map_vec.iter().filter(|&&b| b != UNREFERENCED).count();

    let elapsed = t0.elapsed();
    tracing::info!(
        elapsed_ms = elapsed.as_millis() as u64,
        assigned,
        unreferenced = page_count - assigned,
        conflicts = final_conflicts,
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

    let result = WalkResult { owner_map: owner_map_vec, page_count, page_size: ps, tree_info };

    save_cache(path, file_size, best_txnid, &result);

    mmap.evict();

    Ok(result)
}
