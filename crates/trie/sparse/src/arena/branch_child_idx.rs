// This module is intentionally left minimal after the hot/cold split.
//
// The old `BranchChildIdx` and `BranchChildIter` types were used to map nibbles
// to dense indices in a `SmallVec<[ArenaSparseNodeBranchChild; 4]>`. With the new
// nibble-indexed `[Index; 16]` children array, hot-path lookups no longer need
// dense index computation. The blinded dense index helper is in `nodes.rs`.
