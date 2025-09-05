use std::{
    collections::BTreeSet, f32::INFINITY, f64::consts::LOG10_2, future::Future, pin::Pin, sync::Arc,
};

use crate::{
    utils::{address_value, topic_value},
    BlockBoundary, FilterError, FilterMapParams, FilterResult, LogIndexProvider, MapValueRows,
};
use alloy_primitives::{map::HashMap, B256};
use alloy_rpc_types_eth::Filter;
use futures::{
    future::try_join_all,
    stream::{ForEachConcurrent, FuturesOrdered},
    StreamExt,
};
use itertools::Itertools;
use tokio::{task, time::Instant};
use tracing::{info, trace};

const ADDRESS_OFFSET: u64 = 0;
const TOPIC_OFFSET_BASE: u64 = 1;

/// Default concurrency for parallel processing
const DEFAULT_PARALLEL_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy)]
struct MapRangeChunk {
    start: u32,
    end: u32,
    epoch: u32,
}

/// Calculate epoch-chunked map ranges for the block boundaries using params only
fn calculate_epoch_chunks(
    params: &FilterMapParams,
    block_boundaries: &[BlockBoundary],
) -> Option<Vec<MapRangeChunk>> {
    if block_boundaries.is_empty() {
        return None;
    }

    let first_lv = block_boundaries.first()?.log_value_index;
    let last_lv = block_boundaries.last()?.log_value_index;

    // map index = floor(lv_index / values_per_map) == lv_index >> log_values_per_map
    let map_start = (first_lv >> params.log_values_per_map) as u32;
    let map_end = (last_lv >> params.log_values_per_map) as u32;

    let start_epoch = params.map_epoch(map_start);
    let end_epoch = params.map_epoch(map_end);

    let mut chunks = Vec::with_capacity((end_epoch - start_epoch + 1) as usize);
    for epoch in start_epoch..=end_epoch {
        let start = if epoch == start_epoch { map_start } else { params.first_epoch_map(epoch) };
        let end = if epoch == end_epoch { map_end } else { params.last_epoch_map(epoch) };
        chunks.push(MapRangeChunk { start, end, epoch });
    }
    Some(chunks)
}

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
fn fetch_block_boundaries<P: LogIndexProvider>(
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

fn fetch_filter_rows<P: LogIndexProvider>(
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

            all_matches.extend(matches);
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
    let Some((first_offset, first_values)) = constraints.first().cloned() else {
        return Ok(Vec::new()); // This should never happen
    };
    let first_values = first_values.clone();

    let mut candidates: BTreeSet<u64> = first_values
        .iter()
        .flat_map(|value| {
            get_matches_for_constraint(params, value, first_offset, map_start, map_end, rows_by_map)
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

        // Intersect
        let delta = offset - first_offset;
        candidates.retain(|&c| {
            c.checked_add(delta).map_or(false, |shifted| current_matches.contains(&shifted))
        });
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
    P: LogIndexProvider,
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

/// Query logs in a block range in parallel.
pub async fn spawn_query_logs_tasks<P>(
    provider: Arc<P>,
    params: FilterMapParams,
    filter: Filter,
    from_block: u64,
    to_block: u64,
    concurrency: usize,
) -> FilterResult<
    FuturesOrdered<
        Pin<Box<dyn Future<Output = Result<Vec<FilterMapQueryResult>, FilterError>> + Send>>,
    >,
>
where
    P: LogIndexProvider + Send + Sync + 'static,
{
    let mut handles = FuturesOrdered::new();

    let log_value_indices = fetch_block_boundaries(provider.as_ref(), from_block, to_block)?;

    let (first_map, last_map) =
        match calculate_map_range(&log_value_indices, params.log_values_per_map) {
            Some(r) => r,
            None => return Ok(handles),
        };

    let maps = (first_map..last_map).collect::<Vec<_>>();

    let chunk_size = std::cmp::max(maps.len() / concurrency, 1);

    let chunks = maps
        .into_iter()
        .chunks(chunk_size)
        .into_iter()
        .map(|chunk| chunk.collect::<Vec<_>>())
        .collect::<Vec<_>>();

    let has_constraints = !filter.address.is_empty() || filter.topics.iter().any(|t| !t.is_empty());
    if !has_constraints {
        return Err(FilterError::NoConstraints);
    }

    let values = extract_all_filter_values(&filter);

    for chunk in chunks {
        let provider = Arc::clone(&provider);
        let params = params.clone();
        let filter = filter.clone();
        let values = values.clone();
        let log_value_indices = log_value_indices.clone();

        let map_start = *chunk.first().unwrap();
        let map_end = *chunk.last().unwrap();

        let chunk_task = Box::pin(async move {
            let chunk_task = task::spawn_blocking(move || -> Vec<FilterMapQueryResult> {
                let rows = provider
                    .get_rows_until_short_row(map_start, map_end, &values)
                    .unwrap_or(Vec::new());
                let mut rows_by_map = HashMap::default();
                for row in rows {
                    rows_by_map.insert((row.map_index, row.value), row);
                }

                let matches = query_maps_range(&params, map_start, map_end, &filter, &rows_by_map)
                    .unwrap_or(Vec::new());
                resolve_to_blocks(matches, &log_value_indices, from_block, to_block)
            });

            match chunk_task.await {
                Ok(chunk_results) => Ok(chunk_results),
                Err(join_err) => {
                    trace!(target: "rpc::eth::filter", error = ?join_err, "Task join error");
                    Err(FilterError::Task("Join error".to_string()))
                }
            }
        });

        handles.push_back(chunk_task);
    }

    Ok(handles)
}
