use std::collections::BTreeSet;

use crate::{
    utils::{address_value, topic_value},
    BlockBoundary, FilterError, FilterMapParams, FilterMapsReader, FilterResult, MapValueRows,
};
use alloy_primitives::{map::HashMap, B256};
use alloy_rpc_types_eth::Filter;
use tracing::info;

const ADDRESS_OFFSET: u64 = 0;
const TOPIC_OFFSET_BASE: u64 = 1;

/// Result from a filter map query containing indices and metadata
#[derive(Debug, Clone)]
pub struct FilterMapQueryResult {
    /// Log value index from the filter map
    pub log_index: u64,
    /// Block number containing this log
    pub block_number: u64,
    /// Starting log value index for this block (for position calculation)
    pub block_start_lv_index: u64,
}

/// Fetch log indices for the block range
fn fetch_block_boundaries<P: FilterMapsReader>(
    provider: &P,
    from_block: u64,
    to_block: u64,
) -> FilterResult<Vec<BlockBoundary>> {
    if from_block > to_block {
        return Err(FilterError::InvalidRange(from_block, to_block));
    }

    provider.get_log_value_indices_range(from_block..=to_block)
}

/// Calculate the map range for the block boundaries
fn calculate_map_range(
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

/// Extract all unique values from filter (addresses and topics)
fn extract_all_filter_values(filter: &Filter) -> Vec<B256> {
    let mut values = Vec::new();

    // Add address values
    values.extend(filter.address.iter().map(address_value));

    // Add topic values
    for topics in &filter.topics {
        values.extend(topics.iter().map(topic_value));
    }

    values
}

/// Build constraints list from filter
fn build_constraints(filter: &Filter) -> Vec<(u64, Vec<B256>)> {
    let mut constraints = Vec::new();

    // Add address constraint if present
    if !filter.address.is_empty() {
        let addresses: Vec<B256> = filter.address.iter().map(address_value).collect();
        constraints.push((ADDRESS_OFFSET, addresses));
    }

    // Add topic constraints
    for (pos, topics) in filter.topics.iter().enumerate() {
        if !topics.is_empty() {
            let topic_values: Vec<B256> = topics.iter().map(topic_value).collect();
            constraints.push((pos as u64 + TOPIC_OFFSET_BASE, topic_values));
        }
    }

    constraints
}

fn fetch_filter_rows<P: FilterMapsReader>(
    provider: &P,
    map_start: u32,
    map_end: u32,
    filter: &Filter,
) -> FilterResult<HashMap<(u32, B256), MapValueRows>> {
    let values = extract_all_filter_values(filter);
    let rows = provider.get_rows_until_short_row(map_start, map_end, &values)?;

    let mut rows_map = HashMap::default();
    for row in rows {
        rows_map.insert((row.map_index, row.value), row);
    }

    Ok(rows_map)
}

/// Get all matches for a constraint value across multiple maps
fn get_matches_for_constraint(
    params: &FilterMapParams,
    value: &B256,
    position_offset: u64, // 0 for address, 1+ for topics
    map_start: u32,
    map_end: u32,
    rows_by_map: &HashMap<(u32, B256), MapValueRows>,
) -> Vec<u64> {
    let mut all_matches = Vec::new();

    for map_index in map_start..=map_end {
        if let Some(rows) = rows_by_map.get(&(map_index, *value)) {
            let matches = params.potential_matches(&rows.layers, map_index, value);

            // Adjust indices to point to log start (address position)
            let adjusted: Vec<u64> =
                matches.into_iter().filter_map(|m| m.checked_sub(position_offset)).collect();

            all_matches.extend(adjusted);
        }
    }

    all_matches
}

/// Resolve log indices to blocks with metadata
fn resolve_to_blocks(
    log_indices: Vec<u64>,
    log_value_indices: &[BlockBoundary],
    from_block: u64,
    to_block: u64,
) -> Vec<FilterMapQueryResult> {
    let mut results = Vec::new();

    for log_index in log_indices {
        // Binary search to find the block containing this log index
        let block_idx =
            match log_value_indices.binary_search_by_key(&log_index, |lv| lv.log_value_index) {
                Ok(idx) => idx,
                Err(0) => continue,  // Before first block
                Err(idx) => idx - 1, // Log belongs to previous block
            };

        let block_number = log_value_indices[block_idx].block_number;

        // Verify block is in requested range
        if block_number < from_block || block_number > to_block {
            info!("Skipping log_index {} in block {} (outside range)", log_index, block_number);
            continue;
        }

        let block_start_lv_index = log_value_indices[block_idx].log_value_index;

        results.push(FilterMapQueryResult { log_index, block_number, block_start_lv_index });
    }

    results
}

/// Query logs across a range of maps
fn query_maps_range(
    params: &FilterMapParams,
    map_start: u32,
    map_end: u32,
    filter: &Filter,
    rows_by_map: &HashMap<(u32, B256), MapValueRows>,
) -> FilterResult<Vec<u64>> {
    let constraints = build_constraints(filter);

    if constraints.is_empty() {
        return Ok(Vec::new());
    }

    // Start with first constraint as baseline
    let Some((first_offset, first_values)) = constraints.first() else {
        return Ok(Vec::new()); // This should never happen
    };
    let first_values = first_values.clone();

    let mut candidates: BTreeSet<u64> = first_values
        .iter()
        .flat_map(|value| {
            get_matches_for_constraint(
                params,
                value,
                *first_offset,
                map_start,
                map_end,
                rows_by_map,
            )
        })
        .collect();

    // Intersect with remaining constraints
    for (offset, values) in constraints.into_iter().skip(1) {
        if candidates.is_empty() {
            break;
        }

        let current_matches: BTreeSet<u64> = values
            .iter()
            .flat_map(|value| {
                get_matches_for_constraint(params, value, offset, map_start, map_end, rows_by_map)
            })
            .collect();

        // Intersect in-place
        candidates.retain(|candidate| current_matches.contains(candidate));
    }

    Ok(candidates.into_iter().collect())
}

/// Query logs from filter maps for a given block range
pub fn query_logs_in_block_range<P>(
    provider: &P,
    params: &FilterMapParams,
    filter: &Filter,
    from_block: u64,
    to_block: u64,
) -> FilterResult<Vec<FilterMapQueryResult>>
where
    P: FilterMapsReader,
{
    let log_value_indices = fetch_block_boundaries(provider, from_block, to_block)?;
    let (first_map, last_map) =
        match calculate_map_range(&log_value_indices, params.log_values_per_map) {
            Some(r) => r,
            None => return Ok(vec![]),
        };

    // Check if filter has any constraints we can use
    let has_constraints = !filter.address.is_empty() || filter.topics.iter().any(|t| !t.is_empty());
    if !has_constraints {
        return Err(FilterError::NoConstraints);
    }

    let rows_by_map = fetch_filter_rows(provider, first_map, last_map, filter)?;

    // Query all maps at once
    let matches = query_maps_range(params, first_map, last_map, filter, &rows_by_map)?;

    let results = resolve_to_blocks(matches, &log_value_indices, from_block, to_block);

    Ok(results)
}
