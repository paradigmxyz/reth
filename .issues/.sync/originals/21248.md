---
title: 'SPaC:  Add `calculate_v2_proofs` to SparseTrieInterface'
labels:
    - A-trie
    - C-enhancement
assignees:
    - yongkangc
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 21246
synced_at: 2026-01-21T11:32:16.021303Z
info:
    author: mediocregopher
    created_at: 2026-01-21T10:38:20Z
    updated_at: 2026-01-21T10:47:05Z
---

### Describe the feature

## Summary                                                                                                                                                                                     
                                                                                                                                                                                               
Add a `calculate_v2_proofs` method to `SparseTrieInterface` that accepts a set of changed leaf keys and returns `MultiProofTargetsV2` with appropriate `min_len` values based on the deepest already-revealed nodes in the trie. This enables the sparse trie to request only the trie nodes it doesn't already have.                                                                          
                                                                                                                                                                                               
## Details                                                                                                                                                                                     
                                                                                                                                                                                               
### Method Signature                                                                                                                                                                           
                                                                                                                                                                                               
Add to `SparseTrieInterface` trait in `crates/trie/sparse/src/traits.rs`:                                                                                                                      
                                                                                                                                                                                               
```rust                                                                                                                                                                                        
/// Calculates V2 proof targets for the given leaf keys based on revealed state.                                                                                                               
///                                                                                                                                                                                            
/// For each leaf key, traverses the trie to find the deepest revealed node                                                                                                                    
/// along that key's path, then sets `min_len` to that path length + 1.                                                                                                                        
/// This ensures proofs only fetch nodes beyond what's already revealed.                                                                                                                       
///                                                                                                                                                                                            
/// # Arguments                                                                                                                                                                                
///                                                                                                                                                                                            
/// * `leaf_keys` - Iterator of leaf keys that need proofs                                                                                                                        
///                                                                                                                                                                                            
/// # Returns                                                                                                                                                                                  
///                                                                                                                                                                                            
/// `MultiProofTargetsV2` containing targets with appropriate `min_len` values.                                                                                                                
fn calculate_v2_proof_targets<'a>(                                                                                                                                                             
    &self,                                                                                                                                                                                     
    leaf_keys: impl IntoIterator<Item = &'a B256>,                                                                                                                                          
) -> MultiProofTargetsV2;                                                                                                                                                                      
``` 

### Implementation Requirements                                                                                                                                                                
                                                                                                                                                                                               
1. **Efficient Traversal**: The implementation should not restart from root for each leaf key. When multiple leaves share a common prefix (same sub-trie), the traversal should reuse work. Consider:                                                                                                 
   - Sorting leaf keys lexicographically first (or requiring them to already be sorted)                                                                   
   - Tracking the current traversal position                                                                       
   - Only backtracking when necessary for a new sub-trie
   - Parallelism: the ParallelSparseTrie can use rayon to work on multiple sub-tries at once, see its `reveal_nodes` implementation.                                                                          
                                                                                                        
2. **Finding Deepest Revealed Node**: For each leaf key, walk down the trie following the key's nibbles. Track the path length of the deepest node that is:
   - Revealed (not a `Hash` variant / blinded node)
   - Along the path to the target leaf

3. **Setting `min_len`**: Set `min_len` to `deepest_revealed_path_length + 1`. This tells the proof calculator to skip nodes at depths ≤ the revealed path length.

4. **Handle Edge Cases**:
   - If no nodes are revealed along the path (root is blinded), `min_len` should be 0
   - If the leaf itself is already revealed, we do not need to return a target
   - Empty trie: no need to return any targets

### SparseStateTrie Integration

Also add a method to `SparseStateTrie` in `crates/trie/sparse/src/state.rs`:

```rust
/// Calculates V2 proof targets for account and storage changes.
///
/// # Arguments
///
/// * `state` - The hashed post state containing account and storage changes
///
/// # Returns
///
/// `MultiProofTargetsV2` with appropriate `min_len` values for all targets.
pub fn calculate_v2_proof_targets(&self, state: &HashedPostState) -> MultiProofTargetsV2;
```

This method should:
1. Extract account keys from `state.accounts` and call `calculate_v2_proof_targets` on the account trie
2. For each storage in `state.storages`, get the corresponding storage trie and call `calculate_v2_proof_targets` on it
3. Combine results into a single `MultiProofTargetsV2`

### Additional context

_No response_
