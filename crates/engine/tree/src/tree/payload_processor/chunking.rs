//! Generic chunk planning utilities for multiproof scheduling in the engine.

/// A computed plan for splitting `total` items into `chunks` parts.
///
/// Sizes are distributed as evenly as possible:
/// - `base` items per chunk
/// - the first `increased` chunks get one extra item (size `base + 1`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChunkPlan {
    /// Number of chunks to produce.
    pub chunks: usize,
    /// Base size for each chunk.
    pub base: usize,
    /// Number of leading chunks that receive an extra item.
    pub increased: usize,
}

impl ChunkPlan {
    /// Returns the size for the `index`-th chunk (0-based).
    #[cfg(test)]
    pub(super) fn chunk_size(&self, index: usize) -> usize {
        self.base + usize::from(index < self.increased)
    }

    /// Returns an iterator of chunk sizes with length `self.chunks`.
    #[cfg(test)]
    pub(super) fn sizes(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.chunks).map(|i| self.chunk_size(i))
    }
}

/// Computes a chunk plan given the total amount of items, the number of idle workers
/// available to process chunks concurrently, and a minimum chunk size.
pub(super) fn compute_chunk_plan(total: usize, idle: usize, min_chunk: usize) -> Option<ChunkPlan> {
    if idle == 0 || total == 0 {
        return None;
    }

    let max_chunks_amount = total / min_chunk;
    let chunks = max_chunks_amount.min(idle);

    (chunks >= 2).then(|| {
        let base = total / chunks;
        let increased = total % chunks;
        ChunkPlan { chunks, base, increased }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_idle_no_plan() {
        assert_eq!(compute_chunk_plan(10, 0, 1), None);
        assert_eq!(compute_chunk_plan(0, 4, 1), None);
    }

    #[test]
    fn chunks_respect_min_chunk_and_idle() {
        // total=10, idle=3, min_chunk=4 -> max_chunks=2 -> chunks=2 -> base=5, increased=0
        let plan = compute_chunk_plan(10, 3, 4).unwrap();
        assert_eq!(plan, ChunkPlan { chunks: 2, base: 5, increased: 0 });
        assert_eq!(plan.sizes().collect::<Vec<_>>(), vec![5, 5]);
    }

    #[test]
    fn remainder_distributed_to_leading_chunks() {
        // total=10, idle=4, min_chunk=3 -> max_chunks=3 -> chunks=3 -> base=3, increased=1
        let plan = compute_chunk_plan(10, 4, 3).unwrap();
        assert_eq!(plan, ChunkPlan { chunks: 3, base: 3, increased: 1 });
        assert_eq!(plan.sizes().collect::<Vec<_>>(), vec![4, 3, 3]);
    }

    #[test]
    fn no_chunk_if_only_one_possible() {
        // total=5, idle=8, min_chunk=3 -> max_chunks=1 -> chunks=1 -> None
        assert_eq!(compute_chunk_plan(5, 8, 3), None);
    }
}
