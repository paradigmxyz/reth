use crate::constraints::Constraints;
use alloy_primitives::BlockNumber;
use alloy_rpc_types_eth::Filter;
use reth_log_index_common::{BlockBoundary, LogIndexParams, MAX_LAYERS};
use reth_storage_api::LogIndexProvider;
use reth_storage_errors::provider::ProviderResult;
use std::collections::HashMap;
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
pub fn resolve_to_blocks(
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
pub fn query_maps_range<P: LogIndexProvider>(
    provider: &P,
    params: &LogIndexParams,
    map_start: u32,
    map_end: u32,
    filter: &Filter,
) -> ProviderResult<Vec<u64>> {
    let constraints = Constraints::from_filter(filter);
    let active_maps: Vec<u32> = (map_start..=map_end).collect();
    if active_maps.is_empty() {
        return Ok(Vec::new());
    }

    let mut instance = constraints.new_instance(provider, params, active_maps.clone());
    let mut results: HashMap<u32, Option<Vec<u64>>> = HashMap::with_capacity(active_maps.len());

    for layer in 0..MAX_LAYERS as u32 {
        if results.len() == active_maps.len() || !instance.has_pending() {
            break;
        }

        let layer_results = instance.get_matches_for_layer(layer)?;
        for result in layer_results {
            results
                .entry(result.map_index)
                .and_modify(|existing| {
                    if result.matches.is_none() {
                        *existing = None;
                    }
                })
                .or_insert(result.matches);
        }
    }

    let mut candidates = Vec::new();
    for map_index in active_maps {
        match results.get(&map_index) {
            Some(Some(matches)) => candidates.extend(matches.iter().copied()),
            Some(None) => {
                info!("Wildcard constraint detected; returning empty candidate set");
                return Ok(Vec::new());
            }
            None => {}
        }
    }

    candidates.sort_unstable();
    candidates.dedup();
    Ok(candidates)
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
