use std::collections::{HashMap, HashSet};

use alloy_primitives::B256;
use alloy_rpc_types_eth::Filter;

use crate::utils::{address_value, topic_value};
use reth_log_index_common::{LogIndexParams, MatcherResult, MAX_LAYERS};
use reth_storage_api::LogIndexProvider;
use reth_storage_errors::provider::ProviderResult;

/// High-level matcher constructed from filter constraints.
#[derive(Clone, Debug)]
pub(crate) struct Constraints {
    matcher: Matcher,
}

impl Constraints {
    pub(crate) fn from_filter(filter: &Filter) -> Self {
        let mut matchers = Vec::new();

        let addresses: Vec<B256> = filter.address.iter().map(address_value).collect();
        matchers.push(Matcher::from_values(addresses));

        for topics in &filter.topics {
            let values: Vec<B256> = topics.iter().map(topic_value).collect();
            matchers.push(Matcher::from_values(values));
        }

        let matcher = Matcher::sequence(matchers);
        Self { matcher }
    }

    pub(crate) fn new_instance<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        map_indices: Vec<u32>,
    ) -> MatcherInstance<'a, P> {
        self.matcher.new_instance(provider, params, map_indices)
    }
}

#[derive(Clone, Debug)]
enum Matcher {
    Single(SingleMatcher),
    Any(MatchAny),
    Sequence(Box<MatchSequence>),
    Wildcard,
}

impl Matcher {
    fn from_values(values: Vec<B256>) -> Self {
        if values.is_empty() {
            Matcher::Wildcard
        } else if values.len() == 1 {
            Matcher::Single(SingleMatcher { value: values.into_iter().next().unwrap() })
        } else {
            let children = values.into_iter().map(|value| Matcher::Single(SingleMatcher { value })).collect();
            Matcher::Any(MatchAny { children })
        }
    }

    fn sequence(mut matchers: Vec<Matcher>) -> Self {
        match matchers.len() {
            0 => Matcher::Wildcard,
            1 => matchers.remove(0),
            len => {
                let next = matchers.pop().expect("non-empty matchers");
                let base = Matcher::sequence(matchers);
                Matcher::Sequence(Box::new(MatchSequence { offset: (len - 1) as u64, base, next }))
            }
        }
    }

    fn new_instance<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        map_indices: Vec<u32>,
    ) -> MatcherInstance<'a, P> {
        match self {
            Matcher::Single(single) => MatcherInstance::Single(single.new_instance(provider, params, map_indices)),
            Matcher::Any(any) => any.new_instance(provider, params, map_indices),
            Matcher::Sequence(seq) => MatcherInstance::Sequence(Box::new(seq.new_instance(provider, params, map_indices))),
            Matcher::Wildcard => MatcherInstance::Wildcard(WildcardInstance::new(map_indices)),
        }
    }
}

#[derive(Clone, Debug)]
struct SingleMatcher {
    value: B256,
}

impl SingleMatcher {
    fn new_instance<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        map_indices: Vec<u32>,
    ) -> SingleMatcherInstance<'a, P> {
        SingleMatcherInstance {
            provider,
            params,
            value: &self.value,
            map_indices,
            filter_rows: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct MatchAny {
    children: Vec<Matcher>,
}

impl MatchAny {
    fn new_instance<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        map_indices: Vec<u32>,
    ) -> MatcherInstance<'a, P> {
        if self.children.is_empty() {
            MatcherInstance::Wildcard(WildcardInstance::new(map_indices))
        } else {
            let child_instances = self
                .children
                .iter()
                .map(|child| child.new_instance(provider, params, map_indices.clone()))
                .collect();
            MatcherInstance::Any(MatchAnyInstance::new(map_indices, child_instances))
        }
    }
}

#[derive(Clone, Debug)]
struct MatchSequence {
    offset: u64,
    base: Matcher,
    next: Matcher,
}

impl MatchSequence {
    fn new_instance<'a, P: LogIndexProvider>(
        &'a self,
        provider: &'a P,
        params: &'a LogIndexParams,
        map_indices: Vec<u32>,
    ) -> MatchSequenceInstance<'a, P> {
        let base_instance = self.base.new_instance(provider, params, map_indices.clone());
        let next_instance = self.next.new_instance(provider, params, map_indices.clone());
        MatchSequenceInstance::new(
            params,
            self.offset,
            Box::new(base_instance),
            Box::new(next_instance),
            map_indices,
        )
    }
}

pub(crate) enum MatcherInstance<'a, P: LogIndexProvider> {
    Single(SingleMatcherInstance<'a, P>),
    Any(MatchAnyInstance<'a, P>),
    Sequence(Box<MatchSequenceInstance<'a, P>>),
    Wildcard(WildcardInstance),
}

impl<'a, P: LogIndexProvider> MatcherInstance<'a, P> {
    pub(crate) fn get_matches_for_layer(&mut self, layer: u32) -> ProviderResult<Vec<MatcherResult>> {
        match self {
            MatcherInstance::Single(instance) => instance.get_matches_for_layer(layer),
            MatcherInstance::Any(instance) => instance.get_matches_for_layer(layer),
            MatcherInstance::Sequence(instance) => instance.get_matches_for_layer(layer),
            MatcherInstance::Wildcard(instance) => Ok(instance.get_matches_for_layer()),
        }
    }

    pub(crate) fn drop_indices(&mut self, indices: &[u32]) {
        match self {
            MatcherInstance::Single(instance) => instance.drop_indices(indices),
            MatcherInstance::Any(instance) => instance.drop_indices(indices),
            MatcherInstance::Sequence(instance) => instance.drop_indices(indices),
            MatcherInstance::Wildcard(instance) => instance.drop_indices(indices),
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        match self {
            MatcherInstance::Single(instance) => instance.has_pending(),
            MatcherInstance::Any(instance) => instance.has_pending(),
            MatcherInstance::Sequence(instance) => instance.has_pending(),
            MatcherInstance::Wildcard(instance) => instance.has_pending(),
        }
    }
}

pub(crate) struct SingleMatcherInstance<'a, P: LogIndexProvider> {
    provider: &'a P,
    params: &'a LogIndexParams,
    value: &'a B256,
    map_indices: Vec<u32>,
    filter_rows: HashMap<u32, Vec<Vec<u32>>>,
}

impl<'a, P: LogIndexProvider> SingleMatcherInstance<'a, P> {
    fn get_matches_for_layer(&mut self, layer: u32) -> ProviderResult<Vec<MatcherResult>> {
        if self.map_indices.is_empty() {
            return Ok(Vec::new());
        }

        let rows = self.provider.get_rows_for_value_layer(self.value, &self.map_indices, layer)?;
        let mut results = Vec::new();
        let max_len = self.params.max_row_length(layer) as usize;

        for (map_index, row) in rows {
            let mut aggregated = self.filter_rows.remove(&map_index).unwrap_or_default();
            aggregated.push(row.clone());

            if row.len() < max_len || layer + 1 >= MAX_LAYERS as u32 {
                let matches = self.params.potential_matches(&aggregated, map_index, self.value);
                results.push(MatcherResult { map_index, matches: Some(matches) });
            } else {
                self.filter_rows.insert(map_index, aggregated);
            }
        }

        self.clean_map_indices();
        Ok(results)
    }

    fn drop_indices(&mut self, indices: &[u32]) {
        if indices.is_empty() {
            return;
        }

        let mut retain = Vec::with_capacity(self.map_indices.len());
        let drop_set: HashSet<u32> = indices.iter().copied().collect();
        for idx in &self.map_indices {
            if !drop_set.contains(idx) {
                retain.push(*idx);
            }
        }
        self.map_indices = retain;
        for index in indices {
            self.filter_rows.remove(index);
        }
    }

    fn has_pending(&self) -> bool {
        !self.map_indices.is_empty()
    }

    fn clean_map_indices(&mut self) {
        self.map_indices.retain(|idx| self.filter_rows.contains_key(idx));
    }
}

pub(crate) struct MatchAnyInstance<'a, P: LogIndexProvider> {
    map_indices: Vec<u32>,
    child_instances: Vec<MatcherInstance<'a, P>>,
    child_results: HashMap<u32, MatchAnyResults>,
}

impl<'a, P: LogIndexProvider> MatchAnyInstance<'a, P> {
    fn new(map_indices: Vec<u32>, child_instances: Vec<MatcherInstance<'a, P>>) -> Self {
        let mut child_results = HashMap::new();
        for &idx in &map_indices {
            child_results.insert(
                idx,
                MatchAnyResults {
                    matches: vec![None; child_instances.len()],
                    done: vec![false; child_instances.len()],
                    need_more: child_instances.len(),
                },
            );
        }
        Self { map_indices, child_instances, child_results }
    }

    fn get_matches_for_layer(&mut self, layer: u32) -> ProviderResult<Vec<MatcherResult>> {
        if self.child_instances.is_empty() {
            let results = self
                .map_indices
                .iter()
                .map(|&map_index| MatcherResult { map_index, matches: None })
                .collect();
            self.map_indices.clear();
            self.child_results.clear();
            return Ok(results);
        }

        let mut merged_results = Vec::new();

        for (idx, instance) in self.child_instances.iter_mut().enumerate() {
            let results = instance.get_matches_for_layer(layer)?;
            for result in results {
                if let Some(state) = self.child_results.get_mut(&result.map_index) {
                    if state.done[idx] {
                        continue;
                    }
                    state.done[idx] = true;
                    state.matches[idx] = result.matches.clone();
                    state.need_more = state.need_more.saturating_sub(1);
                    if state.matches[idx].is_none() {
                        state.need_more = 0;
                    }
                    if state.need_more == 0 {
                        let merged = merge_results(&state.matches);
                        merged_results.push(MatcherResult { map_index: result.map_index, matches: merged });
                        self.child_results.remove(&result.map_index);
                    }
                }
            }
        }

        Ok(merged_results)
    }

    fn drop_indices(&mut self, indices: &[u32]) {
        for child in &mut self.child_instances {
            child.drop_indices(indices);
        }
        for index in indices {
            self.child_results.remove(index);
        }
        let drop_set: HashSet<u32> = indices.iter().copied().collect();
        self.map_indices.retain(|idx| !drop_set.contains(idx));
    }

    fn has_pending(&self) -> bool {
        !self.child_results.is_empty()
            || self.child_instances.iter().any(MatcherInstance::has_pending)
    }
}

#[derive(Clone)]
struct MatchAnyResults {
    matches: Vec<Option<Vec<u64>>>,
    done: Vec<bool>,
    need_more: usize,
}

pub(crate) struct WildcardInstance {
    map_indices: Vec<u32>,
    emitted: bool,
}

impl WildcardInstance {
    fn new(map_indices: Vec<u32>) -> Self {
        Self { map_indices, emitted: false }
    }

    fn get_matches_for_layer(&mut self) -> Vec<MatcherResult> {
        if self.emitted {
            return Vec::new();
        }
        self.emitted = true;
        self.map_indices
            .iter()
            .map(|&map_index| MatcherResult { map_index, matches: None })
            .collect()
    }

    fn drop_indices(&mut self, indices: &[u32]) {
        let drop_set: HashSet<u32> = indices.iter().copied().collect();
        self.map_indices.retain(|idx| !drop_set.contains(idx));
    }

    fn has_pending(&self) -> bool {
        !self.map_indices.is_empty() && !self.emitted
    }
}

#[derive(Default, Clone, Copy)]
struct MatchOrderStats {
    total_count: u64,
    non_empty_count: u64,
    total_cost: u64,
}

impl MatchOrderStats {
    fn add(&mut self, empty: bool, layer: u32) {
        if empty && layer != 0 {
            return;
        }
        self.total_count += 1;
        if !empty {
            self.non_empty_count += 1;
        }
        self.total_cost += (layer + 1) as u64;
    }

    fn merge(&mut self, other: MatchOrderStats) {
        self.total_count += other.total_count;
        self.non_empty_count += other.non_empty_count;
        self.total_cost += other.total_cost;
    }
}

pub(crate) struct MatchSequenceInstance<'a, P: LogIndexProvider> {
    params: &'a LogIndexParams,
    offset: u64,
    base: Box<MatcherInstance<'a, P>>,
    next: Box<MatcherInstance<'a, P>>,
    need_matched: HashSet<u32>,
    base_requested: HashSet<u32>,
    next_requested: HashSet<u32>,
    base_results: HashMap<u32, Option<Vec<u64>>>,
    next_results: HashMap<u32, Option<Vec<u64>>>,
    base_stats: MatchOrderStats,
    next_stats: MatchOrderStats,
}

impl<'a, P: LogIndexProvider> MatchSequenceInstance<'a, P> {
    fn new(
        params: &'a LogIndexParams,
        offset: u64,
        base: Box<MatcherInstance<'a, P>>,
        next: Box<MatcherInstance<'a, P>>,
        map_indices: Vec<u32>,
    ) -> Self {
        let need_matched: HashSet<u32> = map_indices.iter().copied().collect();
        let base_requested = need_matched.clone();
        let next_requested = need_matched.clone();
        Self {
            params,
            offset,
            base,
            next,
            need_matched,
            base_requested,
            next_requested,
            base_results: HashMap::new(),
            next_results: HashMap::new(),
            base_stats: MatchOrderStats::default(),
            next_stats: MatchOrderStats::default(),
        }
    }

    fn get_matches_for_layer(&mut self, layer: u32) -> ProviderResult<Vec<MatcherResult>> {
        let base_first = self.choose_base_first();
        if base_first {
            self.eval_base(layer)?;
            self.eval_next(layer)?;
        } else {
            self.eval_next(layer)?;
            self.eval_base(layer)?;
        }

        let mut ready = Vec::new();
        for &map_index in &self.need_matched {
            if self.base_requested.contains(&map_index) || self.next_requested.contains(&map_index) {
                continue;
            }
            ready.push(map_index);
        }

        let mut results = Vec::new();
        for map_index in ready {
            let base_opt = self.base_results.get(&map_index).and_then(|opt| opt.as_ref().map(|v| v.as_slice()));
            let next_opt = self.next_results.get(&map_index).and_then(|opt| opt.as_ref().map(|v| v.as_slice()));
            let matches = self.params.match_results(map_index, self.offset, base_opt, next_opt);
            results.push(MatcherResult { map_index, matches });
            self.need_matched.remove(&map_index);
            self.base_results.remove(&map_index);
            self.next_results.remove(&map_index);
        }

        Ok(results)
    }

    fn drop_indices(&mut self, indices: &[u32]) {
        if indices.is_empty() {
            return;
        }

        self.base.as_mut().drop_indices(indices);
        self.next.as_mut().drop_indices(indices);
        for index in indices {
            self.need_matched.remove(index);
            self.base_requested.remove(index);
            self.next_requested.remove(index);
            self.base_results.remove(index);
            self.next_results.remove(index);
        }
    }

    fn has_pending(&self) -> bool {
        !self.need_matched.is_empty()
            || self.base.as_ref().has_pending()
            || self.next.as_ref().has_pending()
    }

    fn choose_base_first(&self) -> bool {
        let base = self.base_stats;
        let next = self.next_stats;
        let lhs = (base.total_cost as f64) * (next.total_count as f64)
            + (base.non_empty_count as f64) * (next.total_cost as f64);
        let rhs = (base.total_cost as f64) * (next.non_empty_count as f64)
            + (next.total_cost as f64) * (base.total_count as f64);
        lhs < rhs
    }

    fn eval_base(&mut self, layer: u32) -> ProviderResult<()> {
        if self.base_requested.is_empty() {
            return Ok(());
        }

        let results = self.base.as_mut().get_matches_for_layer(layer)?;
        let mut stats = MatchOrderStats::default();
        let mut drop_indices = Vec::new();
        for result in results {
            if let Some(ref matches) = result.matches {
                stats.add(matches.is_empty(), layer);
            }
            self.base_results.insert(result.map_index, result.matches.clone());
            self.base_requested.remove(&result.map_index);
            if self.drop_next_if_possible(result.map_index) {
                drop_indices.push(result.map_index);
            }
        }
        self.base_stats.merge(stats);
        if !drop_indices.is_empty() {
            self.next.as_mut().drop_indices(&drop_indices);
        }
        Ok(())
    }

    fn eval_next(&mut self, layer: u32) -> ProviderResult<()> {
        if self.next_requested.is_empty() {
            return Ok(());
        }

        let results = self.next.as_mut().get_matches_for_layer(layer)?;
        let mut stats = MatchOrderStats::default();
        let mut drop_indices = Vec::new();
        for result in results {
            if let Some(ref matches) = result.matches {
                stats.add(matches.is_empty(), layer);
            }
            self.next_results.insert(result.map_index, result.matches.clone());
            self.next_requested.remove(&result.map_index);
            if self.drop_base_if_possible(result.map_index) {
                drop_indices.push(result.map_index);
            }
        }
        self.next_stats.merge(stats);
        if !drop_indices.is_empty() {
            self.base.as_mut().drop_indices(&drop_indices);
        }
        Ok(())
    }

    fn drop_next_if_possible(&mut self, map_index: u32) -> bool {
        if !self.next_requested.contains(&map_index) {
            return false;
        }
        if self.need_matched.contains(&map_index) {
            match self.base_results.get(&map_index) {
                Some(Some(matches)) if matches.is_empty() => {}
                Some(None) | Some(Some(_)) | None => return false,
            }
        }
        self.next_requested.remove(&map_index);
        true
    }

    fn drop_base_if_possible(&mut self, map_index: u32) -> bool {
        if !self.base_requested.contains(&map_index) {
            return false;
        }
        if self.need_matched.contains(&map_index) {
            match self.next_results.get(&map_index) {
                Some(Some(matches)) if matches.is_empty() => {}
                Some(None) | Some(Some(_)) | None => return false,
            }
        }
        self.base_requested.remove(&map_index);
        true
    }
}

fn merge_results(results: &[Option<Vec<u64>>]) -> Option<Vec<u64>> {
    if results.iter().any(|r| r.is_none()) {
        return None;
    }

    let mut combined = Vec::new();
    for res in results {
        if let Some(values) = res {
            combined.extend_from_slice(values);
        }
    }
    combined.sort_unstable();
    combined.dedup();
    Some(combined)
}
