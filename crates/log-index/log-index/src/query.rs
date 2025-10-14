use crate::constraints::Constraints;
use std::collections::HashSet;
use alloy_primitives::BlockNumber;
use alloy_rpc_types_eth::Filter;
use reth_log_index_common::{BlockBoundary, LogIndexParams};
use reth_storage_api::LogIndexProvider;
use reth_storage_errors::provider::ProviderResult;
use tracing::info;

/// Fetch log indices for the block range
fn fetch_block_boundaries<P: LogIndexProvider>(
    provider: &P,
    from_block: u64,
    to_block: u64,
) -> ProviderResult<Vec<BlockBoundary>> {
    if from_block > to_block {
        // TODO: Implement error handling for invalid block range
    }

    provider.get_log_value_indices_range(from_block..=to_block)
}

/// Calculate the map range for the block boundaries
pub fn calculate_map_range(
    block_boundaries: &[BlockBoundary],
    log_values_per_map: u32,
) -> Option<(u32, u32)> {
    if block_boundaries.is_empty() {
        return None;
    }

    let first_lv = block_boundaries.first()?.log_value_index;
    let last_lv = block_boundaries.last()?.log_value_index;

    let first_map = (first_lv >> log_values_per_map) as u32;
    let last_map = (last_lv >> log_values_per_map) as u32;

    Some((first_map, last_map))
}

/// Resolve log indices to blocks with metadata
fn resolve_to_blocks(
    log_indices: Vec<u64>,
    log_value_indices: &[BlockBoundary],
    from_block: u64,
    to_block: u64,
) -> Vec<BlockNumber> {
    let mut results = Vec::new();

    for log_index in log_indices {
        // Binary search to find the block containing this log index
        let block_idx =
            match log_value_indices.binary_search_by_key(&log_index, |lv| lv.log_value_index) {
                Ok(idx) => idx,
                Err(0) => {
                    continue;
                }
                Err(idx) => {
                    idx - 1 // Log might belong to previous block TODO: We can continue here, if we
                            // are sure that the log index we are searching for is outside of the
                            // range of the log indices we are searching for
                }
            };

        let block_number = log_value_indices[block_idx].block_number;

        // Verify block is in requested range
        if block_number < from_block || block_number > to_block {
            continue;
        }

        results.push(block_number);
    }

    results.sort_unstable();
    results.dedup();

    results
}

/// Query logs across a range of maps
fn query_maps_range<P: LogIndexProvider>(
    provider: &P,
    params: &LogIndexParams,
    map_start: u32,
    map_end: u32,
    filter: &Filter,
) -> ProviderResult<Vec<u64>> {
    let constraints = Constraints::from_filter(filter);

    let mut active_maps: Vec<u32> = (map_start..=map_end).collect();
    let mut candidates: Vec<u64> = Vec::new();
    let mut first_offset: Option<u64> = None;

    for constraint in constraints.iter() {
        info!("Processing constraint: {:?}", constraint);

        let mut layer_iter = constraint.layers(provider, params, active_maps.clone());
        let previous_offset = first_offset;
        let previous_candidates = first_offset.map(|_| candidates.as_slice());
        let mut constraint_matches: Vec<u64> = Vec::new();
        let mut aligned_candidates: Vec<u64> = Vec::new();

        while let Some(result) = layer_iter.next() {
            let (overflow_maps, matches) = result?;
            info!(
                "Got result with matches: {:?}, overflow maps: {:?}",
                matches.len(),
                overflow_maps.len()
            );
            if matches.is_empty() && overflow_maps.is_empty() {
                info!("No matches found for constraint");
                return Ok(Vec::new());
            }

            constraint_matches = union_sorted(&constraint_matches, &matches);

            if let Some(offset) = previous_offset {
                let delta = constraint.offset() as i64 - offset as i64;
                if delta == 0 {
                    info!("Unioning matches for constraint offset {}", constraint.offset());
                } else {
                    info!("Intersecting layer candidates with offset {}", delta);
                }

                if let Some(prev_candidates) = previous_candidates {
                    let aligned = intersect_with_offset_slice(prev_candidates, &matches, delta);
                    if aligned.is_empty() {
                        info!("Layer candidates empty after intersecting; continuing to next layer");
                    } else {
                        aligned_candidates = union_sorted(&aligned_candidates, &aligned);
                    }
                }
            }

            if overflow_maps.is_empty() {
                info!("No overflow maps found for constraint");
                break;
            }

            if overflow_maps.is_empty() {
                info!("No overflow maps remain for constraint");
                break;
            }

            let mut next_maps = overflow_maps;

            if let Some(prev_candidates) = previous_candidates {
                if prev_candidates.is_empty() {
                    info!("No previous candidates available to pursue overflow maps");
                    break;
                }

                let prev_map_set: HashSet<u32> = prev_candidates
                    .iter()
                    .map(|&idx| (idx >> params.log_values_per_map) as u32)
                    .collect();

                next_maps.retain(|map| prev_map_set.contains(map));

                if next_maps.is_empty() {
                    info!("Overflow maps do not intersect with previous candidates");
                    break;
                }
            }

            layer_iter.set_maps(next_maps);

        }

        let updated_candidates =
            if previous_offset.is_some() { aligned_candidates } else { constraint_matches };

        if updated_candidates.is_empty() {
            info!("No matches found for constraint at the end");
            return Ok(Vec::new());
        }

        if let Some(offset) = previous_offset {
            let delta = constraint.offset() as i64 - offset as i64;
            if delta == 0 {
                info!("Unioning matches for constraint offset {}", constraint.offset());
            } else {
                info!("Intersecting with offset {}", delta);
            }
            candidates = updated_candidates;
        } else {
            candidates = updated_candidates;
            first_offset = Some(constraint.offset());
        }

        active_maps =
            candidates.iter().map(|&idx| (idx >> params.log_values_per_map) as u32).collect();
        active_maps.sort_unstable();
        active_maps.dedup();
    }

    Ok(candidates)
}

fn intersect_with_offset_slice(candidates: &[u64], matches: &[u64], delta: i64) -> Vec<u64> {
    if candidates.is_empty() || matches.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;

    while i < candidates.len() && j < matches.len() {
        let shifted = if delta >= 0 {
            candidates[i].checked_add(delta as u64)
        } else {
            let diff = (-delta) as u64;
            candidates[i].checked_sub(diff)
        };

        let Some(shifted) = shifted else {
            i += 1;
            continue;
        };

        match shifted.cmp(&matches[j]) {
            std::cmp::Ordering::Less => {
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                result.push(candidates[i]);
                i += 1;
                j += 1;
            }
        }
    }

    result
}

fn union_sorted(a: &[u64], b: &[u64]) -> Vec<u64> {
    if a.is_empty() {
        return b.to_vec();
    }
    if b.is_empty() {
        return a.to_vec();
    }

    let mut result = Vec::with_capacity(a.len() + b.len());
    let mut i = 0usize;
    let mut j = 0usize;

    while i < a.len() && j < b.len() {
        let value = if a[i] < b[j] {
            let v = a[i];
            i += 1;
            v
        } else if b[j] < a[i] {
            let v = b[j];
            j += 1;
            v
        } else {
            let v = a[i];
            i += 1;
            j += 1;
            v
        };

        if result.last().copied() != Some(value) {
            result.push(value);
        }
    }

    while i < a.len() {
        let value = a[i];
        i += 1;
        if result.last().copied() != Some(value) {
            result.push(value);
        }
    }

    while j < b.len() {
        let value = b[j];
        j += 1;
        if result.last().copied() != Some(value) {
            result.push(value);
        }
    }

    result
}

/// Query logs from filter maps for a given block range
pub fn query_logs_in_block_range<P>(
    provider: &P,
    params: &LogIndexParams,
    filter: &Filter,
    from_block: u64,
    to_block: u64,
) -> ProviderResult<Vec<BlockNumber>>
where
    P: LogIndexProvider,
{
    let log_value_indices = fetch_block_boundaries(provider, from_block, to_block)?;
    let (first_map, last_map) =
        match calculate_map_range(&log_value_indices, params.log_values_per_map) {
            Some(r) => r,
            None => return Ok(vec![]),
        };

    // Query all maps at once
    let matches = query_maps_range(provider, params, first_map, last_map, filter)?;

    info!("Got matches: {:?}", matches.len());

    let results = resolve_to_blocks(matches, &log_value_indices, from_block, to_block);

    Ok(results)
}
