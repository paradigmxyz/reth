//! Mapping between spec IDs and their respective EVM version.

use revm::primitives::{spec_to_generic, Spec, SpecId};
use std::sync::OnceLock;

/// Converts the given `SpecId` to the corresponding EVM version.
#[inline]
pub fn spec_id_to_evm_version(spec_id: SpecId) -> SpecId {
    spec_to_generic!(spec_id, SPEC::SPEC_ID)
}

/// Returns `true` if the given `SpecId` is an EVM version.
#[inline]
pub fn is_evm_version(spec_id: SpecId) -> bool {
    spec_id_to_evm_version(spec_id) == spec_id
}

#[derive(Clone, Copy)]
pub(crate) struct EvmVersions(pub(crate) &'static Vec<SpecId>);

impl EvmVersions {
    /// All `SpecId`s except for >=prague and =latest.
    #[inline]
    pub(crate) fn enabled() -> Self {
        static ALL: OnceLock<Vec<SpecId>> = OnceLock::new();
        Self(ALL.get_or_init(|| {
            // EOF is not supported.
            (0..SpecId::PRAGUE as u8)
                .filter_map(SpecId::try_from_u8)
                .map(spec_id_to_evm_version)
                .collect()
        }))
    }

    /// Returns the first EVM version.
    pub(crate) fn first() -> SpecId {
        Self::enabled().0.first().copied().unwrap()
    }

    /// Returns the last EVM version.
    pub(crate) fn last() -> SpecId {
        Self::enabled().0.last().copied().unwrap()
    }

    #[inline]
    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    #[inline]
    pub(crate) fn iter(self) -> EvmVersionIterator<'static> {
        EvmVersionIterator::new(self.0)
    }

    #[inline]
    pub(crate) fn iter_starting_at(self, start: SpecId) -> EvmVersionIterator<'static> {
        EvmVersionIterator::new(&self.0[start as usize..])
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn get(self, spec_id: SpecId) -> Option<SpecId> {
        self.0.get(spec_id as usize).copied()
    }
}

impl IntoIterator for EvmVersions {
    type Item = SpecId;
    type IntoIter = EvmVersionIterator<'static>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl std::ops::Index<SpecId> for EvmVersions {
    type Output = SpecId;

    #[inline]
    fn index(&self, index: SpecId) -> &Self::Output {
        self.0.index(index as usize)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EvmVersionIterator<'a> {
    current: Option<SpecId>,
    iter: std::slice::Iter<'a, SpecId>,
}

impl<'a> EvmVersionIterator<'a> {
    #[inline]
    fn new(all_evm_versions: &'a [SpecId]) -> Self {
        Self { current: None, iter: all_evm_versions.iter() }
    }
}

impl Iterator for EvmVersionIterator<'_> {
    type Item = SpecId;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        for &next in self.iter.by_ref() {
            if Some(next) != self.current {
                self.current = Some(next);
                return Some(next);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted() {
        let all = EvmVersions::enabled();
        assert!(all.0.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn no_eof() {
        let all = EvmVersions::enabled();
        assert!(all.0.iter().all(|v| *v < SpecId::PRAGUE));
    }

    #[test]
    fn mapped() {
        let all = EvmVersions::enabled();
        for v in all.iter() {
            assert_eq!(spec_id_to_evm_version(v), v);
            assert_eq!(all[v], v);
        }
        assert_eq!(all.get(SpecId::FRONTIER_THAWING), Some(SpecId::FRONTIER));
        assert!(all.get(SpecId::LATEST).is_none());
        assert!(all.iter().all(|v| v != SpecId::LATEST));
    }

    #[test]
    fn iter_all() {
        let all = EvmVersions::enabled();
        let mut all_deduped = all.0.clone();
        all_deduped.dedup();
        let mut iter = all.iter();
        assert_eq!(iter.clone().collect::<Vec<_>>(), all_deduped);
        assert_eq!(all.iter_starting_at(SpecId::FRONTIER_THAWING).collect::<Vec<_>>(), all_deduped);
        assert_eq!(iter.next(), Some(SpecId::FRONTIER));
        assert_eq!(iter.next(), Some(SpecId::HOMESTEAD));
    }
}
