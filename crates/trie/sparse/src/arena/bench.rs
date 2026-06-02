use super::{
    ArenaParallelSparseTrie, ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeBranchChild,
    ArenaSparseNodeState, ArenaSparseSubtrie, BranchChildIdx, Index, NodeArena,
};
use crate::{LeafUpdate, SparseTrie};
use alloy_primitives::B256;
use alloy_trie::TrieMask;
use rand::{seq::SliceRandom, SeedableRng};
use reth_trie_common::{BranchNodeMasks, LeafNode, Nibbles, ProofTrieNodeV2, RlpNode, TrieNodeV2};
use smallvec::SmallVec;
use std::{env, hint::black_box, io::Write, time::Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BenchRunMode {
    Both,
    Scalar,
    Interleaved,
}

impl BenchRunMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Scalar => "scalar",
            Self::Interleaved => "interleaved",
        }
    }

    const fn includes_scalar(self) -> bool {
        matches!(self, Self::Both | Self::Scalar)
    }

    const fn includes_interleaved(self) -> bool {
        matches!(self, Self::Both | Self::Interleaved)
    }
}

#[derive(Debug, Clone, Copy)]
struct ArenaBenchLookup {
    head: Index,
    path: Nibbles,
}

#[derive(Debug, Clone, Copy)]
enum InterleavedLookupPhase {
    LoadCurrent,
    UsePrefetched { child: Index, path_offset: usize },
}

#[derive(Debug, Clone, Copy)]
struct InterleavedLookupTask {
    current: Index,
    path: Nibbles,
    path_offset: usize,
    phase: InterleavedLookupPhase,
}

impl InterleavedLookupTask {
    const fn new(lookup: ArenaBenchLookup) -> Self {
        Self {
            current: lookup.head,
            path: lookup.path,
            path_offset: 0,
            phase: InterleavedLookupPhase::LoadCurrent,
        }
    }

    fn step(&mut self, arena: &NodeArena) -> Option<usize> {
        loop {
            match self.phase {
                InterleavedLookupPhase::LoadCurrent => match &arena[self.current] {
                    ArenaSparseNode::Leaf { key, value, .. } => {
                        let remaining = self.path.slice(self.path_offset..);
                        return Some((remaining == *key) as usize * (value.len() + 1));
                    }
                    ArenaSparseNode::Branch(branch) => {
                        let logical_end = self.path_offset + branch.short_key.len();
                        if self.path.len() <= logical_end ||
                            self.path.slice(self.path_offset..logical_end) != branch.short_key
                        {
                            return Some(0);
                        }

                        let child_nibble = self.path.get_unchecked(logical_end);
                        let Some(child_idx) = BranchChildIdx::new(branch.state_mask, child_nibble)
                        else {
                            return Some(0);
                        };

                        let ArenaSparseNodeBranchChild::Revealed(child) =
                            branch.children[child_idx]
                        else {
                            return Some(0);
                        };

                        arena.prefetch(child);
                        self.phase = InterleavedLookupPhase::UsePrefetched {
                            child,
                            path_offset: logical_end + 1,
                        };
                        return None;
                    }
                    ArenaSparseNode::EmptyRoot |
                    ArenaSparseNode::Subtrie(_) |
                    ArenaSparseNode::TakenSubtrie => return Some(0),
                },
                InterleavedLookupPhase::UsePrefetched { child, path_offset } => {
                    self.current = child;
                    self.path_offset = path_offset;
                    self.phase = InterleavedLookupPhase::LoadCurrent;
                }
            }
        }
    }
}

fn scalar_bench_lookup(arena: &NodeArena, lookups: &[ArenaBenchLookup]) -> usize {
    let mut checksum = 0usize;
    for lookup in lookups {
        let mut current = lookup.head;
        let mut path_offset = 0;
        loop {
            match &arena[current] {
                ArenaSparseNode::Leaf { key, value, .. } => {
                    let remaining = lookup.path.slice(path_offset..);
                    checksum ^= black_box((remaining == *key) as usize * (value.len() + 1));
                    break;
                }
                ArenaSparseNode::Branch(branch) => {
                    let logical_end = path_offset + branch.short_key.len();
                    if lookup.path.len() <= logical_end ||
                        lookup.path.slice(path_offset..logical_end) != branch.short_key
                    {
                        break;
                    }

                    let child_nibble = lookup.path.get_unchecked(logical_end);
                    let Some(child_idx) = BranchChildIdx::new(branch.state_mask, child_nibble)
                    else {
                        break;
                    };

                    let ArenaSparseNodeBranchChild::Revealed(child) = branch.children[child_idx]
                    else {
                        break;
                    };

                    current = child;
                    path_offset = logical_end + 1;
                }
                ArenaSparseNode::EmptyRoot |
                ArenaSparseNode::Subtrie(_) |
                ArenaSparseNode::TakenSubtrie => break,
            }
        }
    }
    checksum
}

fn interleaved_bench_lookup(
    arena: &NodeArena,
    lookups: &[ArenaBenchLookup],
    group_size: usize,
) -> usize {
    let mut checksum = 0usize;
    let mut next_lookup = 0;
    let mut active = 0usize;
    let mut slots = Vec::with_capacity(group_size);

    for _ in 0..group_size {
        let slot = lookups.get(next_lookup).copied().map(InterleavedLookupTask::new);
        if slot.is_some() {
            active += 1;
            next_lookup += 1;
        }
        slots.push(slot);
    }

    while active > 0 {
        for slot in &mut slots {
            let Some(task) = slot.as_mut() else { continue };
            let Some(result) = task.step(arena) else { continue };

            checksum ^= black_box(result);
            if let Some(lookup) = lookups.get(next_lookup).copied() {
                *slot = Some(InterleavedLookupTask::new(lookup));
                next_lookup += 1;
            } else {
                *slot = None;
                active -= 1;
            }
        }
    }

    checksum
}

fn build_synthetic_lookup_arena(
    num_chains: usize,
    depth: usize,
) -> (NodeArena, Vec<ArenaBenchLookup>) {
    let mut arena = NodeArena::with_capacity(num_chains.saturating_mul(depth + 1));
    let mut paths = Vec::with_capacity(num_chains);
    let mut current = Vec::with_capacity(num_chains);

    for chain in 0..num_chains {
        let mut path = Nibbles::default();
        for level in 0..depth {
            path.push_unchecked(bench_nibble(chain, level));
        }
        paths.push(path);
        current.push(arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Revealed,
            value: Vec::new(),
            key: Nibbles::default(),
        }));
    }

    for level in (0..depth).rev() {
        let mut next = Vec::with_capacity(num_chains);
        for (chain, child) in current.into_iter().enumerate() {
            let child_nibble = bench_nibble(chain, level);
            let state_mask = TrieMask::from(1u16 << child_nibble);
            let mut children = SmallVec::with_capacity(1);
            children.push(ArenaSparseNodeBranchChild::Revealed(child));
            next.push(arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                state: ArenaSparseNodeState::Revealed,
                children,
                state_mask,
                short_key: Nibbles::default(),
                branch_masks: BranchNodeMasks::default(),
            })));
        }
        current = next;
    }

    let mut lookups: Vec<_> = current
        .into_iter()
        .zip(paths)
        .map(|(head, path)| ArenaBenchLookup { head, path })
        .collect();
    lookups.shuffle(&mut rand::rngs::StdRng::seed_from_u64(0xA11C_E5EED));

    (arena, lookups)
}

fn build_synthetic_touched_subtrie(
    num_paths: usize,
    depth: usize,
) -> (Box<ArenaSparseSubtrie>, Vec<(B256, Nibbles, LeafUpdate)>) {
    assert!(depth > 0, "depth must be non-zero");

    let mut paths = Vec::with_capacity(num_paths);
    for chain in 0..num_paths {
        let mut path = Nibbles::default();
        for level in 0..depth {
            path.push_unchecked(bench_nibble(chain, level));
        }
        paths.push(path);
    }
    paths.sort_unstable();
    paths.dedup();

    let mut arena = NodeArena::with_capacity(paths.len().saturating_mul(depth));
    let blinded = RlpNode::word_rlp(&B256::ZERO);
    let root = build_synthetic_layered_root(&mut arena, &paths, depth, |_| {
        ArenaSparseNodeBranchChild::Blinded(blinded.clone())
    });

    let updates = paths
        .into_iter()
        .map(|path| {
            (ArenaParallelSparseTrie::nibbles_to_padded_b256(&path), path, LeafUpdate::Touched)
        })
        .collect::<Vec<_>>();

    let mut subtrie = ArenaSparseSubtrie::new(false);
    subtrie.arena = arena;
    subtrie.root = root;
    (subtrie, updates)
}

fn build_synthetic_reveal_subtrie(
    num_paths: usize,
    depth: usize,
) -> (Box<ArenaSparseSubtrie>, Vec<ProofTrieNodeV2>) {
    assert!(depth > 0, "depth must be non-zero");

    let mut paths = Vec::with_capacity(num_paths);
    for chain in 0..num_paths {
        let mut path = Nibbles::default();
        for level in 0..depth {
            path.push_unchecked(bench_nibble(chain, level));
        }
        paths.push(path);
    }
    paths.sort_unstable();
    paths.dedup();

    let leaf = LeafNode::new(Nibbles::default(), Vec::new());
    let leaf_rlp = RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap();

    let mut arena = NodeArena::with_capacity(paths.len().saturating_mul(depth));
    let root = build_synthetic_layered_root(&mut arena, &paths, depth, |_| {
        ArenaSparseNodeBranchChild::Blinded(leaf_rlp.clone())
    });

    let nodes = paths
        .into_iter()
        .map(|path| ProofTrieNodeV2 { path, node: TrieNodeV2::Leaf(leaf.clone()), masks: None })
        .collect::<Vec<_>>();

    let mut subtrie = ArenaSparseSubtrie::new(false);
    subtrie.arena = arena;
    subtrie.root = root;
    (subtrie, nodes)
}

fn build_synthetic_changed_subtrie(
    num_paths: usize,
    depth: usize,
) -> (Box<ArenaSparseSubtrie>, Vec<(B256, Nibbles, LeafUpdate)>) {
    assert!(depth > 0, "depth must be non-zero");

    let mut paths = Vec::with_capacity(num_paths);
    for chain in 0..num_paths {
        let mut path = Nibbles::default();
        for level in 0..depth {
            path.push_unchecked(bench_nibble(chain, level));
        }
        paths.push(path);
    }
    paths.sort_unstable();
    paths.dedup();

    let mut arena = NodeArena::with_capacity(paths.len().saturating_mul(depth + 1));
    let root = build_synthetic_layered_root(&mut arena, &paths, depth, |arena| {
        ArenaSparseNodeBranchChild::Revealed(arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Revealed,
            value: Vec::new(),
            key: Nibbles::default(),
        }))
    });

    let updates = paths
        .into_iter()
        .map(|path| {
            (
                ArenaParallelSparseTrie::nibbles_to_padded_b256(&path),
                path,
                LeafUpdate::Changed(vec![0x01]),
            )
        })
        .collect::<Vec<_>>();

    let mut subtrie = ArenaSparseSubtrie::new(false);
    subtrie.arena = arena;
    subtrie.root = root;
    subtrie.num_leaves = updates.len() as u64;
    (subtrie, updates)
}

fn build_synthetic_removal_subtrie(
    num_groups: usize,
    depth: usize,
) -> (Box<ArenaSparseSubtrie>, Vec<(B256, Nibbles, LeafUpdate)>) {
    assert!(depth > 0, "depth must be non-zero");

    let mut parent_paths = Vec::with_capacity(num_groups);
    for group in 0..num_groups {
        let mut path = Nibbles::default();
        for level in 0..depth - 1 {
            path.push_unchecked(bench_nibble(group, level));
        }
        parent_paths.push(path);
    }
    parent_paths.sort_unstable();
    parent_paths.dedup();

    let mut paths = Vec::with_capacity(parent_paths.len() * 16);
    let mut updates = Vec::with_capacity(parent_paths.len());
    for parent_path in parent_paths {
        for nibble in 0..16 {
            let mut path = parent_path;
            path.push_unchecked(nibble);
            if nibble == 0 {
                updates.push((
                    ArenaParallelSparseTrie::nibbles_to_padded_b256(&path),
                    path,
                    LeafUpdate::Changed(Vec::new()),
                ));
            }
            paths.push(path);
        }
    }

    let mut arena = NodeArena::with_capacity(paths.len().saturating_mul(depth + 1));
    let root = build_synthetic_layered_root(&mut arena, &paths, depth, |arena| {
        ArenaSparseNodeBranchChild::Revealed(arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Revealed,
            value: Vec::new(),
            key: Nibbles::default(),
        }))
    });

    let mut subtrie = ArenaSparseSubtrie::new(false);
    subtrie.arena = arena;
    subtrie.root = root;
    subtrie.num_leaves = paths.len() as u64;
    (subtrie, updates)
}

fn build_synthetic_value_trie(
    num_paths: usize,
    depth: usize,
) -> (ArenaParallelSparseTrie, Vec<Nibbles>) {
    assert!(depth > 0, "depth must be non-zero");

    let mut paths = Vec::with_capacity(num_paths);
    for chain in 0..num_paths {
        let mut path = Nibbles::default();
        for level in 0..depth {
            path.push_unchecked(bench_nibble(chain, level));
        }
        paths.push(path);
    }
    paths.sort_unstable();
    paths.dedup();

    let mut arena = NodeArena::with_capacity(paths.len().saturating_mul(depth + 1));
    let root = build_synthetic_layered_root(&mut arena, &paths, depth, |arena| {
        ArenaSparseNodeBranchChild::Revealed(arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Revealed,
            value: Vec::new(),
            key: Nibbles::default(),
        }))
    });

    let mut subtrie = ArenaSparseSubtrie::new(false);
    subtrie.arena = arena;
    subtrie.root = root;
    subtrie.num_leaves = paths.len() as u64;

    let mut trie = ArenaParallelSparseTrie::default();
    trie.upper_arena = NodeArena::new();
    trie.root = trie.upper_arena.insert(ArenaSparseNode::Subtrie(subtrie));

    paths.shuffle(&mut rand::rngs::StdRng::seed_from_u64(0xA11C_E5EED));

    (trie, paths)
}

fn build_synthetic_layered_root(
    arena: &mut NodeArena,
    paths: &[Nibbles],
    depth: usize,
    mut terminal_child: impl FnMut(&mut NodeArena) -> ArenaSparseNodeBranchChild,
) -> Index {
    debug_assert!(!paths.is_empty());
    let mut current = paths.iter().map(|path| (*path, terminal_child(arena))).collect::<Vec<_>>();

    for level in (0..depth).rev() {
        let mut next = Vec::new();
        let mut start = 0;
        while start < current.len() {
            let parent_prefix = current[start].0.slice(..level);
            let mut end = start + 1;
            while end < current.len() && current[end].0.starts_with(&parent_prefix) {
                end += 1;
            }

            let mut children = SmallVec::new();
            let mut state_mask = TrieMask::default();
            for (path, child) in &current[start..end] {
                let child_nibble = path.get_unchecked(level);
                state_mask.set_bit(child_nibble);
                children.push(child.clone());
            }

            let branch = ArenaSparseNodeBranch {
                state: ArenaSparseNodeState::Revealed,
                children,
                state_mask,
                short_key: Nibbles::default(),
                branch_masks: BranchNodeMasks::default(),
            };
            next.push((
                parent_prefix,
                ArenaSparseNodeBranchChild::Revealed(arena.insert(ArenaSparseNode::Branch(branch))),
            ));
            start = end;
        }
        current = next;
    }

    let (_, root) = current.pop().expect("root exists");
    let ArenaSparseNodeBranchChild::Revealed(root) = root else { unreachable!() };
    root
}

fn bench_nibble(chain: usize, level: usize) -> u8 {
    let mixed =
        (chain as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(((level * 7) & 63) as u32) ^
            (level as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    ((mixed >> 60) & 0x0F) as u8
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
}

fn bench_run_mode() -> BenchRunMode {
    match env::var("RETH_ARENA_AMAC_BENCH_MODE").as_deref() {
        Ok("scalar") => BenchRunMode::Scalar,
        Ok("interleaved") | Ok("batched") => BenchRunMode::Interleaved,
        Ok("both") | Err(_) => BenchRunMode::Both,
        Ok(mode) => panic!("unknown RETH_ARENA_AMAC_BENCH_MODE: {mode}"),
    }
}

fn pause_before_timed_phase(label: &str, mode: BenchRunMode) {
    if env::var_os("RETH_ARENA_AMAC_BENCH_PAUSE").is_none() {
        return;
    }

    eprintln!("{label} ready pid={} mode={}", std::process::id(), mode.as_str());
    std::io::stderr().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}

fn bench_items_for_target(default_items: usize, slots_per_item: usize) -> usize {
    if let Ok(rss_gib) = env::var("RETH_ARENA_AMAC_BENCH_RSS_GIB") {
        let target_bytes =
            rss_gib.parse::<usize>().expect("RSS GiB must be a positive integer") << 30;
        (target_bytes / (slots_per_item * NodeArena::slot_size())).max(1)
    } else {
        env_usize("RETH_ARENA_AMAC_BENCH_CHAINS", default_items)
    }
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_interleaved_arena_lookup() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 16);
    let group_size = env_usize("RETH_ARENA_AMAC_BENCH_GROUP", 64).max(1);
    let num_chains = bench_items_for_target(1 << 16, depth + 1);

    let (arena, lookups) = build_synthetic_lookup_arena(num_chains, depth);
    eprintln!(
        "arena_lookup_bench chains={} depth={} group={} arena_memory_mib={:.1}",
        num_chains,
        depth,
        group_size,
        arena.memory_size() as f64 / (1024.0 * 1024.0)
    );

    let started = Instant::now();
    let scalar = scalar_bench_lookup(&arena, &lookups);
    let scalar_elapsed = started.elapsed();

    let started = Instant::now();
    let interleaved = interleaved_bench_lookup(&arena, &lookups, group_size);
    let interleaved_elapsed = started.elapsed();

    assert_eq!(scalar, interleaved);

    let lookups = lookups.len() as f64;
    let scalar_ns = scalar_elapsed.as_nanos() as f64 / lookups;
    let interleaved_ns = interleaved_elapsed.as_nanos() as f64 / lookups;
    eprintln!(
        "arena_lookup_bench scalar_ns_per_lookup={scalar_ns:.1} \
         interleaved_ns_per_lookup={interleaved_ns:.1} speedup={:.2}x",
        scalar_ns / interleaved_ns
    );
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_removed_subtrie_update() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 32);
    let mode = bench_run_mode();
    let num_groups = bench_items_for_target(1 << 14, 16 * (depth + 1));

    let (subtrie, updates) = build_synthetic_removal_subtrie(num_groups, depth);
    eprintln!(
        "removed_update_bench updates={} depth={} arena_memory_mib={:.1}",
        updates.len(),
        depth,
        subtrie.arena.memory_size() as f64 / (1024.0 * 1024.0)
    );

    let update_count = updates.len() as f64;
    let mut scalar_subtrie = mode.includes_scalar().then(|| subtrie.clone());
    if let Some(subtrie) = scalar_subtrie.as_mut() {
        subtrie.buffers.cursor.reset(&subtrie.arena, subtrie.root, subtrie.path);
    }
    let mut interleaved_subtrie = mode.includes_interleaved().then(|| subtrie.clone());

    pause_before_timed_phase("removed_update_bench", mode);

    let scalar = scalar_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_removed_leaves_scalar(&updates, 0..updates.len());
        subtrie.buffers.cursor.drain(&mut subtrie.arena);
        let elapsed = started.elapsed();
        (
            black_box(subtrie.num_leaves),
            subtrie.num_dirty_leaves,
            elapsed.as_nanos() as f64 / update_count,
        )
    });

    let interleaved = interleaved_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_removed_leaves_interleaved(&updates, 0..updates.len());
        let elapsed = started.elapsed();
        (
            black_box(subtrie.num_leaves),
            subtrie.num_dirty_leaves,
            elapsed.as_nanos() as f64 / update_count,
        )
    });

    match (scalar, interleaved) {
        (
            Some((scalar_leaves, scalar_dirty, scalar_ns)),
            Some((interleaved_leaves, interleaved_dirty, interleaved_ns)),
        ) => {
            assert_eq!(scalar_leaves, interleaved_leaves);
            assert_eq!(scalar_dirty, interleaved_dirty);
            eprintln!(
                "removed_update_bench scalar_ns_per_update={scalar_ns:.1} \
                 interleaved_ns_per_update={interleaved_ns:.1} speedup={:.2}x leaves={}",
                scalar_ns / interleaved_ns,
                interleaved_leaves
            );
        }
        (Some((leaves, _, scalar_ns)), None) => {
            eprintln!("removed_update_bench scalar_ns_per_update={scalar_ns:.1} leaves={leaves}");
        }
        (None, Some((leaves, _, interleaved_ns))) => {
            eprintln!(
                "removed_update_bench interleaved_ns_per_update={interleaved_ns:.1} leaves={leaves}"
            );
        }
        (None, None) => unreachable!(),
    }
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_changed_subtrie_update() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 32);
    let mode = bench_run_mode();
    let num_paths = bench_items_for_target(1 << 16, depth + 1);

    let (subtrie, updates) = build_synthetic_changed_subtrie(num_paths, depth);
    eprintln!(
        "changed_update_bench updates={} depth={} arena_memory_mib={:.1}",
        updates.len(),
        depth,
        subtrie.arena.memory_size() as f64 / (1024.0 * 1024.0)
    );

    let update_count = updates.len() as f64;
    let mut scalar_subtrie = mode.includes_scalar().then(|| subtrie.clone());
    if let Some(subtrie) = scalar_subtrie.as_mut() {
        subtrie.buffers.cursor.reset(&subtrie.arena, subtrie.root, subtrie.path);
    }
    let mut interleaved_subtrie = mode.includes_interleaved().then(|| subtrie.clone());

    pause_before_timed_phase("changed_update_bench", mode);

    let scalar = scalar_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_changed_leaves_scalar(&updates, 0..updates.len());
        subtrie.buffers.cursor.drain(&mut subtrie.arena);
        let elapsed = started.elapsed();
        (
            subtrie.num_leaves,
            black_box(subtrie.num_dirty_leaves),
            elapsed.as_nanos() as f64 / update_count,
        )
    });

    let interleaved = interleaved_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_changed_leaves_interleaved(&updates, 0..updates.len());
        let elapsed = started.elapsed();
        (
            subtrie.num_leaves,
            black_box(subtrie.num_dirty_leaves),
            elapsed.as_nanos() as f64 / update_count,
        )
    });

    match (scalar, interleaved) {
        (
            Some((scalar_leaves, scalar_dirty, scalar_ns)),
            Some((interleaved_leaves, interleaved_dirty, interleaved_ns)),
        ) => {
            assert_eq!(scalar_dirty, interleaved_dirty);
            assert_eq!(scalar_leaves, interleaved_leaves);
            eprintln!(
                "changed_update_bench scalar_ns_per_update={scalar_ns:.1} \
                 interleaved_ns_per_update={interleaved_ns:.1} speedup={:.2}x dirty={}",
                scalar_ns / interleaved_ns,
                interleaved_dirty
            );
        }
        (Some((_, dirty, scalar_ns)), None) => {
            eprintln!("changed_update_bench scalar_ns_per_update={scalar_ns:.1} dirty={dirty}");
        }
        (None, Some((_, dirty, interleaved_ns))) => {
            eprintln!(
                "changed_update_bench interleaved_ns_per_update={interleaved_ns:.1} dirty={dirty}"
            );
        }
        (None, None) => unreachable!(),
    }
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_reveal_subtrie() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 32);
    let mode = bench_run_mode();
    let num_paths = bench_items_for_target(1 << 16, depth);

    let (subtrie, nodes) = build_synthetic_reveal_subtrie(num_paths, depth);
    eprintln!(
        "reveal_subtrie_bench nodes={} depth={} arena_memory_mib={:.1}",
        nodes.len(),
        depth,
        subtrie.arena.memory_size() as f64 / (1024.0 * 1024.0)
    );

    let node_count = nodes.len() as f64;
    let mut scalar_subtrie = mode.includes_scalar().then(|| subtrie.clone());
    let mut scalar_nodes = mode.includes_scalar().then(|| nodes.clone());
    let mut interleaved_subtrie = mode.includes_interleaved().then(|| subtrie.clone());
    let mut interleaved_nodes = mode.includes_interleaved().then(|| nodes.clone());

    pause_before_timed_phase("reveal_subtrie_bench", mode);

    let scalar = match (scalar_subtrie.as_mut(), scalar_nodes.as_mut()) {
        (Some(subtrie), Some(nodes)) => {
            let started = Instant::now();
            subtrie.reveal_nodes_scalar(nodes);
            let elapsed = started.elapsed();
            Some((black_box(subtrie.num_leaves), elapsed.as_nanos() as f64 / node_count))
        }
        _ => None,
    };

    let interleaved = match (interleaved_subtrie.as_mut(), interleaved_nodes.as_mut()) {
        (Some(subtrie), Some(nodes)) => {
            let started = Instant::now();
            subtrie.reveal_nodes_interleaved(nodes);
            let elapsed = started.elapsed();
            Some((black_box(subtrie.num_leaves), elapsed.as_nanos() as f64 / node_count))
        }
        _ => None,
    };

    match (scalar, interleaved) {
        (Some((scalar_leaves, scalar_ns)), Some((interleaved_leaves, interleaved_ns))) => {
            assert_eq!(scalar_leaves, interleaved_leaves);
            eprintln!(
                "reveal_subtrie_bench scalar_ns_per_node={scalar_ns:.1} \
                 interleaved_ns_per_node={interleaved_ns:.1} speedup={:.2}x leaves={}",
                scalar_ns / interleaved_ns,
                interleaved_leaves
            );
        }
        (Some((leaves, scalar_ns)), None) => {
            eprintln!("reveal_subtrie_bench scalar_ns_per_node={scalar_ns:.1} leaves={leaves}");
        }
        (None, Some((leaves, interleaved_ns))) => {
            eprintln!(
                "reveal_subtrie_bench interleaved_ns_per_node={interleaved_ns:.1} leaves={leaves}"
            );
        }
        (None, None) => unreachable!(),
    }
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_touched_subtrie_update() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 32);
    let mode = bench_run_mode();
    let num_paths = bench_items_for_target(1 << 16, depth);

    let (subtrie, updates) = build_synthetic_touched_subtrie(num_paths, depth);
    eprintln!(
        "touched_update_bench updates={} depth={} arena_memory_mib={:.1}",
        updates.len(),
        depth,
        subtrie.arena.memory_size() as f64 / (1024.0 * 1024.0)
    );

    let update_count = updates.len() as f64;
    let mut scalar_subtrie = mode.includes_scalar().then(|| subtrie.clone());
    if let Some(subtrie) = scalar_subtrie.as_mut() {
        subtrie.buffers.cursor.reset(&subtrie.arena, subtrie.root, subtrie.path);
    }
    let mut interleaved_subtrie = mode.includes_interleaved().then(|| subtrie.clone());

    pause_before_timed_phase("touched_update_bench", mode);

    let scalar = scalar_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_touched_leaves_scalar(&updates, 0..updates.len());
        subtrie.buffers.cursor.drain(&mut subtrie.arena);
        let elapsed = started.elapsed();
        (black_box(subtrie.required_proofs.len()), elapsed.as_nanos() as f64 / update_count)
    });

    let interleaved = interleaved_subtrie.as_mut().map(|subtrie| {
        let started = Instant::now();
        subtrie.update_touched_leaves_interleaved(&updates, 0..updates.len());
        let elapsed = started.elapsed();
        (black_box(subtrie.required_proofs.len()), elapsed.as_nanos() as f64 / update_count)
    });

    match (scalar, interleaved) {
        (Some((scalar_proofs, scalar_ns)), Some((interleaved_proofs, interleaved_ns))) => {
            assert_eq!(scalar_proofs, interleaved_proofs);
            eprintln!(
                "touched_update_bench scalar_ns_per_update={scalar_ns:.1} \
                 interleaved_ns_per_update={interleaved_ns:.1} speedup={:.2}x proofs={}",
                scalar_ns / interleaved_ns,
                interleaved_proofs
            );
        }
        (Some((proofs, scalar_ns)), None) => {
            eprintln!("touched_update_bench scalar_ns_per_update={scalar_ns:.1} proofs={proofs}");
        }
        (None, Some((proofs, interleaved_ns))) => {
            eprintln!(
                "touched_update_bench interleaved_ns_per_update={interleaved_ns:.1} proofs={proofs}"
            );
        }
        (None, None) => unreachable!(),
    }
}

#[test]
#[ignore = "manual perf benchmark; set RETH_ARENA_AMAC_BENCH_RSS_GIB=4 for a large run"]
fn bench_batched_leaf_value_lookup() {
    let depth = env_usize("RETH_ARENA_AMAC_BENCH_DEPTH", 32);
    let mode = bench_run_mode();
    let num_paths = bench_items_for_target(1 << 16, depth + 1);
    let (trie, paths) = build_synthetic_value_trie(num_paths, depth);

    let arena_memory_size = match &trie.upper_arena[trie.root] {
        ArenaSparseNode::Subtrie(subtrie) => subtrie.arena.memory_size(),
        _ => unreachable!(),
    };
    eprintln!(
        "leaf_value_lookup_bench lookups={} depth={} arena_memory_mib={:.1}",
        paths.len(),
        depth,
        arena_memory_size as f64 / (1024.0 * 1024.0)
    );

    let lookups = paths.len() as f64;
    pause_before_timed_phase("leaf_value_lookup_bench", mode);

    let scalar = mode.includes_scalar().then(|| {
        let started = Instant::now();
        let found =
            paths.iter().filter(|path| black_box(trie.get_leaf_value(path).is_some())).count();
        let elapsed = started.elapsed();
        assert_eq!(found, paths.len());
        (found, elapsed.as_nanos() as f64 / lookups)
    });

    let interleaved = mode.includes_interleaved().then(|| {
        let started = Instant::now();
        let values = trie.get_leaf_values(&paths);
        let elapsed = started.elapsed();
        let found = black_box(values.iter().filter(|value| value.is_some()).count());
        assert_eq!(found, paths.len());
        (found, elapsed.as_nanos() as f64 / lookups)
    });

    match (scalar, interleaved) {
        (Some((scalar_found, scalar_ns)), Some((batched_found, batched_ns))) => {
            assert_eq!(scalar_found, batched_found);
            eprintln!(
                "leaf_value_lookup_bench scalar_ns_per_lookup={scalar_ns:.1} \
                 batched_ns_per_lookup={batched_ns:.1} speedup={:.2}x",
                scalar_ns / batched_ns
            );
        }
        (Some((_, scalar_ns)), None) => {
            eprintln!("leaf_value_lookup_bench scalar_ns_per_lookup={scalar_ns:.1}");
        }
        (None, Some((_, batched_ns))) => {
            eprintln!("leaf_value_lookup_bench batched_ns_per_lookup={batched_ns:.1}");
        }
        (None, None) => unreachable!(),
    }
}
