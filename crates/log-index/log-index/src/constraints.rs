use std::time::Instant;

use alloy_primitives::B256;
use alloy_rpc_types_eth::Filter;
use tracing::info;

use crate::utils::{address_value, topic_value};
use reth_log_index_common::{LogIndexParams, MAX_LAYERS};
use reth_storage_api::LogIndexProvider;
use reth_storage_errors::provider::ProviderResult;

const ADDRESS_OFFSET: u64 = 0;
const TOPIC_OFFSET_BASE: u64 = 1;

/// Identifies which portion of the filter a constraint applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstraintKind {
    Address,
    Topic(usize),
}

impl ConstraintKind {
    #[inline]
    pub(crate) const fn offset(self) -> u64 {
        match self {
            ConstraintKind::Address => ADDRESS_OFFSET,
            ConstraintKind::Topic(index) => TOPIC_OFFSET_BASE + index as u64,
        }
    }
}

/// A single constraint extracted from the filter (address or topic slot).
#[derive(Debug, Clone)]
pub(crate) struct Constraint {
    pub(crate) kind: ConstraintKind,
    pub(crate) values: Vec<B256>,
}

impl Constraint {
    pub(crate) fn new(kind: ConstraintKind, values: Vec<B256>) -> Self {
        Self { kind, values }
    }

    #[inline]
    pub(crate) fn offset(&self) -> u64 {
        self.kind.offset()
    }

    pub(crate) fn layers<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        initial_maps: Vec<u32>,
    ) -> ConstraintLayerIter<'a, P> {
        ConstraintLayerIter { provider, params, values: &self.values, layer: 0, maps: initial_maps }
    }
}

/// Wrapper around the ordered list of constraints derived from a filter.
#[derive(Debug, Clone)]
pub(crate) struct Constraints {
    items: Vec<Constraint>,
}

impl Constraints {
    pub(crate) fn new(items: Vec<Constraint>) -> Self {
        Self { items }
    }

    pub(crate) fn from_filter(filter: &Filter) -> Self {
        let mut items = Vec::new();

        if !filter.address.is_empty() {
            items.push(Constraint::new(
                ConstraintKind::Address,
                filter.address.iter().map(address_value).collect(),
            ));
        }

        for (pos, topics) in filter.topics.iter().enumerate() {
            let topic_values: Vec<B256> = topics.iter().map(topic_value).collect();
            if !topic_values.is_empty() {
                items.push(Constraint::new(ConstraintKind::Topic(pos), topic_values));
            }
        }

        items.sort_by_key(|c| c.values.len());
        Self::new(items)
    }

    #[inline]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Constraint> {
        self.items.iter()
    }
}

pub(crate) struct ConstraintLayerIter<'a, P> {
    provider: &'a P,
    params: &'a LogIndexParams,
    values: &'a [B256],
    layer: u32,
    maps: Vec<u32>,
}

impl<'a, P: LogIndexProvider> Iterator for ConstraintLayerIter<'a, P> {
    type Item = ProviderResult<(Vec<u32>, Vec<u64>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.maps.is_empty() || self.layer >= MAX_LAYERS as u32 {
            info!("No more maps or layers available");
            return None;
        }
        info!("Starting iteration at layer {} with maps {:?}", self.layer, self.maps.len());

        let mut merged_matches: Vec<u64> = Vec::new();
        let mut overflow_maps: Vec<u32> = Vec::new();

        for value in self.values.iter() {
            let fetch_start = Instant::now();
            let rows = match self.provider.get_rows_for_value_layer(value, &self.maps, self.layer) {
                Ok(rows) => rows,
                Err(err) => return Some(Err(err)),
            };
            info!("Fetched {} rows for value {} in {:?}", rows.len(), value, fetch_start.elapsed());

            let mut value_matches: Vec<u64> = Vec::new();

            for (map_index, row) in rows {
                let row_len = row.len();

                if row_len == 0 {
                    continue;
                }

                if row_len >= self.params.max_row_length(self.layer) as usize &&
                    self.layer + 1 < MAX_LAYERS as u32
                {
                    overflow_maps.push(map_index);
                }
                let potentials = self.params.potential_matches(row, map_index, value);
                value_matches = union_sorted(&value_matches, &potentials);
            }

            merged_matches = union_sorted(&merged_matches, &value_matches);
        }

        overflow_maps.sort_unstable();
        overflow_maps.dedup();

        info!(
            "Finished iteration at layer {} with matches: {:?}, overflow maps: {:?}",
            self.layer,
            merged_matches.len(),
            overflow_maps.len()
        );

        self.layer += 1;
        self.maps = overflow_maps.clone();

        Some(Ok((overflow_maps, merged_matches)))
    }
}

impl<'a, P> ConstraintLayerIter<'a, P> {
    pub(crate) fn set_maps(&mut self, maps: Vec<u32>) {
        self.maps = maps;
    }
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
